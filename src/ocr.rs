//! Windows' own OCR (Windows.Media.Ocr, part of Windows 10 and 11: nothing to ship), for the map name. Only the
//! short-lived `--read-map` process uses it, so the overlay never loads it.

use maple_exp::reader::Frame;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// The lines of text Windows reads in `f`; None when it has no OCR engine or reads nothing.
pub fn lines(f: &Frame) -> Option<Vec<String>> {
    let engine = Language::CreateLanguage(&HSTRING::from("en-US"))
        .and_then(|l| OcrEngine::TryCreateFromLanguage(&l))
        .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())
        .ok()?;
    let writer = DataWriter::new().ok()?;
    writer.WriteBytes(&f.bgra).ok()?;
    let bmp = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &writer.DetachBuffer().ok()?,
        BitmapPixelFormat::Bgra8,
        f.w as i32,
        f.h as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .ok()?;
    let result = engine.RecognizeAsync(&bmp).ok()?.get().ok()?;
    let lines = result.Lines().ok()?;
    let out: Vec<String> = (0..lines.Size().ok()?).filter_map(|i| Some(lines.GetAt(i).ok()?.Text().ok()?.to_string())).collect();
    (!out.is_empty()).then_some(out)
}
