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
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
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
    /// the small CPU-readable texture the box is copied into; kept while its size stays the same
    staging: Option<(ID3D11Texture2D, i32, i32)>,
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

    /// The part (x, y, w, h), in window pixels, of a fresh frame; None if no frame arrives soon.
    pub fn grab(&mut self, x: i32, y: i32, w: i32, h: i32) -> Option<Frame> {
        unsafe {
            let size = self.item.Size().ok()?;
            if x < 0 || y < 0 || w <= 0 || h <= 0 || x + w > size.Width || y + h > size.Height {
                return None;
            }
            let pool =
                Direct3D11CaptureFramePool::CreateFreeThreaded(&self.winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size)
                    .ok()?;
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
                let r = self.read_box(&f, x, y, w, h);
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
    ) -> Option<Frame> {
        unsafe {
            let tex: ID3D11Texture2D = frame.Surface().ok()?.cast::<IDirect3DDxgiInterfaceAccess>().ok()?.GetInterface().ok()?;
            if self.staging.as_ref().is_none_or(|(_, sw, sh)| (*sw, *sh) != (w, h)) {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w as u32,
                    Height: h as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t)).ok()?;
                self.staging = Some((t?, w, h));
            }
            let staging = &self.staging.as_ref()?.0;
            let region = D3D11_BOX { left: x as u32, top: y as u32, front: 0, right: (x + w) as u32, bottom: (y + h) as u32, back: 1 };
            self.ctx.CopySubresourceRegion(staging, 0, 0, 0, 0, &tex, 0, Some(&region));
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).ok()?;
            let mut px = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h as usize {
                // (rows are RowPitch apart, which is not always w * 4)
                let src = std::slice::from_raw_parts((map.pData as *const u8).add(row * map.RowPitch as usize), (w * 4) as usize);
                px.extend_from_slice(src);
            }
            self.ctx.Unmap(staging, 0);
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            Some(Frame { w: w as usize, h: h as usize, bgra: px })
        }
    }
}
