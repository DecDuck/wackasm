fn main() {
    // rustc reserves a 1 MiB stack by default, which alone exceeds a tier 1
    // RAM stick (192 KiB). 64 KiB is plenty for the shell.
    println!("cargo:rustc-link-arg=-zstack-size=65536");
}
