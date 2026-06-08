fn main() {
    // Embed the application icon into the Windows executable.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/justquery.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/justquery.ico");
        if let Err(e) = res.compile() {
            // Don't fail the build if the resource compiler is unavailable.
            eprintln!("warning: failed to embed icon resource: {e}");
        }
    }
}
