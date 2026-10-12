use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{dir}/kernel.ld");
    println!("cargo:rerun-if-changed=kernel.ld");
    if env::var_os("CARGO_FEATURE_QEMU_TEST").is_some() {
        test_lzsa_asm(&dir);
    }
}

/// テスト用カーネルだけ: ローダの LZSA2 の展開ルーチン（boot/unlzsa2.inc）を 32 ビットで組んで
/// リンクし、ホストで圧縮した境目のデータを埋め込む（ktest の `lzsa_asm_*`）
fn test_lzsa_asm(dir: &str) {
    let out = env::var("OUT_DIR").unwrap();
    let boot = Path::new(dir).join("../boot");
    let obj = Path::new(&out).join("unlzsa2_test.o");
    let status = Command::new(env::var("NASM").unwrap_or("nasm".into()))
        .arg("-f")
        .arg("elf32")
        .arg("-I")
        .arg(format!("{}/", boot.display()))
        .arg("-o")
        .arg(&obj)
        .arg(boot.join("unlzsa2_test.asm"))
        .status()
        .expect("nasm");
    assert!(status.success(), "nasm failed");
    println!("cargo:rustc-link-arg={}", obj.display());
    println!("cargo:rerun-if-changed=../boot/unlzsa2.inc");
    println!("cargo:rerun-if-changed=../boot/unlzsa2_test.asm");

    // 形: [件数 u32] { [元の大きさ u32][元の CRC32 u32][圧縮後の大きさ u32][圧縮したもの] }...
    // 大きいデータはフロッピーに収まらないので、20KB までのものと、いくつかの大きいもの
    // （長い 0 の並び・複数のフレームと遠いオフセットを使うもの）だけにする
    let mut blob = Vec::new();
    let mut count = 0u32;
    for (name, data) in vibe_lzsa::samples::samples() {
        if data.len() > 20_000 && !matches!(name.as_str(), "zeros-140000" | "mixed-10" | "mixed-20") {
            continue;
        }
        for parser in [vibe_lzsa::Parser::OptimalRep, vibe_lzsa::Parser::Greedy] {
            let opt = vibe_lzsa::Options { parser, ..vibe_lzsa::Options::new(vibe_lzsa::Version::V2) };
            let packed = vibe_lzsa::compress(&data, &opt).unwrap();
            blob.extend_from_slice(&(data.len() as u32).to_le_bytes());
            blob.extend_from_slice(&vibe_pack::crc32(&data).to_le_bytes());
            blob.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            blob.extend_from_slice(&packed);
            count += 1;
        }
    }
    let mut file = count.to_le_bytes().to_vec();
    file.extend_from_slice(&blob);
    std::fs::write(Path::new(&out).join("lzsa_samples.bin"), file).unwrap();
}
