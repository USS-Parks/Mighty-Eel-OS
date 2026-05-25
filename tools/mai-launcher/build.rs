// Compile lamprey-mai.rc (Windows-only) so the resulting lamprey-mai.exe
// carries the Lamprey icon for Explorer, taskbar, and any shortcut that
// derives its icon from the binary itself. Non-Windows targets skip
// the resource step entirely.

fn main() {
    #[cfg(windows)]
    {
        embed_resource::compile("lamprey-mai.rc", embed_resource::NONE);
        println!("cargo:rerun-if-changed=lamprey-mai.rc");
        println!("cargo:rerun-if-changed=../../docs/assets/lamprey-mai.ico");
    }
}
