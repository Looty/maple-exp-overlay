//! Looks at the game window's own picture with Windows Graphics Capture (what OBS' "Windows 10 (1903+)" capture and
//! Discord's window sharing use). It gets what the game drew, so a taskbar or another window on top can't hide the
//! EXP bar, and it works on DirectX windows where PrintWindow comes back blank.
//!
//! The capture session lives only for the length of one look: it is started, one frame is taken, and it is closed
//! again. Between looks nothing is held but the small D3D device, so the two full-window frame buffers a standing
//! session keeps (about 30 MB at 1440p) never stay resident and Windows isn't copying game frames for us all the time.

use maple_exp::reader::Frame;
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

pub struct Wgc {
    hwnd: isize,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    item: GraphicsCaptureItem,
    /// the small CPU-readable texture the box is copied into; kept while its size (and depth) stays the same
    staging: Option<(ID3D11Texture2D, i32, i32, bool)>,
}

/// A half-precision float (as FP16 textures hold them) as an f32.
fn half(b: u16) -> f32 {
    let (sign, exp, man) = ((b >> 15) as u32, ((b >> 10) & 0x1f) as i32, (b & 0x3ff) as f32);
    let v = match exp {
        0 => man / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 {
        -v
    } else {
        v
    }
}

/// Linear light (0..1) to an 8-bit sRGB value.
fn srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

/// An FP16 (scRGB: linear, 1.0 = SDR white) box as 8-bit BGRA, scaled so its lightest channel is white. With HDR on,
/// Windows lifts the game's SDR picture above 1.0; an 8-bit capture then clips the light colours together (the map
/// name's letters and the panel behind them both become pure white), while these values keep them apart.
fn deep_to_bgra(rgba: &[f32]) -> Vec<u8> {
    let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let top = rgba.chunks_exact(4).flat_map(|p| p[..3].iter().map(|&v| clean(v))).fold(0.0f32, f32::max).max(1e-6);
    rgba.chunks_exact(4).flat_map(|p| [srgb(clean(p[2]) / top), srgb(clean(p[1]) / top), srgb(clean(p[0]) / top), 255]).collect()
}

impl Wgc {
    pub fn new(hwnd: isize) -> windows::core::Result<Wgc> {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED); // (already initialised is fine)
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let (device, ctx) = (device.unwrap(), ctx.unwrap());
            let dxgi: IDXGIDevice = device.cast()?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(HWND(hwnd as *mut _))?;
            Ok(Wgc { hwnd, device, ctx, winrt_device, item, staging: None })
        }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// The part (x, y, w, h), in window pixels, of a fresh frame; None if no frame arrives soon. `deep`: taken in 16-bit
    /// float and brought back to 8 bits here (see `deep_to_bgra`), for true colours with HDR on.
    pub fn grab(&mut self, x: i32, y: i32, w: i32, h: i32, deep: bool) -> Option<Frame> {
        unsafe {
            let size = self.item.Size().ok()?;
            if x < 0 || y < 0 || w <= 0 || h <= 0 || x + w > size.Width || y + h > size.Height {
                return None;
            }
            let format = if deep { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&self.winrt_device, format, 1, size).ok()?;
            let session = pool.CreateCaptureSession(&self.item).ok()?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false); // (only Windows 11 / newer 10 builds; ignored elsewhere)
            session.StartCapture().ok()?;
            // the first frame comes with the game's next present
            let mut frame = None;
            for _ in 0..150 {
                if let Ok(f) = pool.TryGetNextFrame() {
                    frame = Some(f);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let out = frame.and_then(|f| {
                let r = self.read_box(&f, x, y, w, h, deep);
                let _ = f.Close();
                r
            });
            let _ = session.Close();
            let _ = pool.Close();
            out
        }
    }

    unsafe fn read_box(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        deep: bool,
    ) -> Option<Frame> {
        unsafe {
            let tex: ID3D11Texture2D = frame.Surface().ok()?.cast::<IDirect3DDxgiInterfaceAccess>().ok()?.GetInterface().ok()?;
            if self.staging.as_ref().is_none_or(|(_, sw, sh, sd)| (*sw, *sh, *sd) != (w, h, deep)) {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w as u32,
                    Height: h as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: if deep { DXGI_FORMAT_R16G16B16A16_FLOAT } else { DXGI_FORMAT_B8G8R8A8_UNORM },
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t)).ok()?;
                self.staging = Some((t?, w, h, deep));
            }
            let staging = &self.staging.as_ref()?.0;
            let region = D3D11_BOX { left: x as u32, top: y as u32, front: 0, right: (x + w) as u32, bottom: (y + h) as u32, back: 1 };
            self.ctx.CopySubresourceRegion(staging, 0, 0, 0, 0, &tex, 0, Some(&region));
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).ok()?;
            let bpp = if deep { 8 } else { 4 };
            let mut raw = Vec::with_capacity((w * h * bpp) as usize);
            for row in 0..h as usize {
                // (rows are RowPitch apart, which is not always w * bytes per pixel)
                let src = std::slice::from_raw_parts((map.pData as *const u8).add(row * map.RowPitch as usize), (w * bpp) as usize);
                raw.extend_from_slice(src);
            }
            self.ctx.Unmap(staging, 0);
            let mut px = if deep {
                let rgba: Vec<f32> = raw.chunks_exact(2).map(|b| half(u16::from_le_bytes([b[0], b[1]]))).collect();
                deep_to_bgra(&rgba)
            } else {
                raw
            };
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            Some(Frame { w: w as usize, h: h as usize, bgra: px })
        }
    }
}
