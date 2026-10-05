// SPDX-License-Identifier: GPL-3.0-only
//! 构建脚本：嵌入 Windows manifest（Per-Monitor V2 DPI 感知）与图标资源。

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    let _ = embed_resource::compile("app.rc", embed_resource::NONE);
}
