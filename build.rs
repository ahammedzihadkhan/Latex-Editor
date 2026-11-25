fn main() {
    println!("cargo:rerun-if-changed=resource.rc");
    println!("cargo:rerun-if-changed=app.ico");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let res_path = format!("{}/resource.res", manifest_dir.replace('\\', "/"));
    println!("cargo:rustc-link-arg={}", res_path);
}

