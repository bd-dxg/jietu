// SPDX-License-Identifier: GPL-3.0-only
//! 构建脚本：嵌入 Windows manifest 与图标资源。
//! release 构建声明 requireAdministrator，dev 构建不提权（见 ADR 0003）。

fn main() {
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let rc = if profile == "release" { "app.rc" } else { "app.dev.rc" };
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.dev.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.dev.manifest");
    let _ = embed_resource::compile(rc, embed_resource::NONE);
}
