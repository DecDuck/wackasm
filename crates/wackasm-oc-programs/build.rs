fn main() {
    // rustc reserves a 1 MiB stack by default, which alone is more RAM than
    // most OpenComputers machines have. Do the same in your own programs:
    // RUSTFLAGS="-C link-arg=-zstack-size=65536"
    println!("cargo:rustc-link-arg=-zstack-size=65536");
}
