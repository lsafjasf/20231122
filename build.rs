use std::path::{Path, PathBuf};

fn main() {
    let out = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .expect("OUT_DIR is set by cargo");
    let link_dir = out.join("link-libs");
    std::fs::create_dir_all(&link_dir).expect("create link-libs");

    let sysroot = "/home/hj/sysroot/root/usr/lib/x86_64-linux-gnu";
    let system = "/usr/lib/x86_64-linux-gnu";
    // The local sysroot has headers and GTK4/GStreamer dev symlinks, while
    // several transitive runtime libraries are provided by the host.
    let links = [
        ("libpangocairo-1.0.so", system, "libpangocairo-1.0.so.0"),
        ("libpango-1.0.so", system, "libpango-1.0.so.0"),
        ("libharfbuzz.so", system, "libharfbuzz.so.0"),
        ("libgdk_pixbuf-2.0.so", system, "libgdk_pixbuf-2.0.so.0"),
        ("libcairo-gobject.so", system, "libcairo-gobject.so.2"),
        ("libcairo.so", system, "libcairo.so.2"),
        ("libvulkan.so", system, "libvulkan.so.1"),
        ("libgio-2.0.so", system, "libgio-2.0.so.0"),
        ("libgobject-2.0.so", system, "libgobject-2.0.so.0"),
        ("libglib-2.0.so", system, "libglib-2.0.so.0"),
        ("libgstreamer-1.0.so", system, "libgstreamer-1.0.so.0"),
        ("libgstbase-1.0.so", system, "libgstbase-1.0.so.0"),
        ("libgstapp-1.0.so", sysroot, "libgstapp-1.0.so.0.2802.0"),
        ("libgraphene-1.0.so", sysroot, "libgraphene-1.0.so.0.1000.8"),
        ("libgtk-4.so", sysroot, "libgtk-4.so.1.2200.4"),
    ];

    for (name, dir, target) in links {
        let target = Path::new(dir).join(target);
        let link = link_dir.join(name);
        if !target.exists() {
            panic!("missing link target {}", target.display());
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link)
            .unwrap_or_else(|e| panic!("link {}: {e}", link.display()));
    }
    println!("cargo:rustc-link-search=native={}", link_dir.display());
}
