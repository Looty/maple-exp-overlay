fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
        slint_build::compile_with_config("ui/settings.slint", config).unwrap();

        // Embed the icon and manifest so Explorer, the taskbar, the tray and Task Manager show the icon and
        // name, and Windows treats the app as per-monitor DPI aware (no bitmap scaling).
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_manifest(MANIFEST);
        res.set("ProductName", "Maple EXP Overlay");
        res.set("FileDescription", "Maple EXP Overlay"); // Task Manager shows this as the app's name
        res.set("InternalName", "maple-exp-overlay");
        res.set("OriginalFilename", "maple-exp-overlay.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed exe resources: {e}");
        }
    }
}

#[cfg(windows)]
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
</assembly>
"#;
