fn main() {
    // rustc reserves a 1 MiB stack by default, which is more than a tier 1
    // RAM stick (192 KiB). The BIOS needs very little.
    println!("cargo:rustc-link-arg=-zstack-size=16384");
}
