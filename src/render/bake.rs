// SPDX-License-Identifier: GPL-3.0-only
//! 导出烘焙（OUT-1/OUT-2）：把标注对象栅格化进裁剪结果图，
//! 以及聚光（Glow）的「整图暗化 + 区域恢复原图亮度」效果。

use tiny_skia::{Pixmap, Transform};

use crate::editor::{Kind, Object, Point};

/// 导出烘焙（OUT-1/OUT-2）：把标注对象栅格化进裁剪结果图。
/// `origin` 是裁剪区域在覆盖层坐标系中的左上角；对象坐标减去它即为裁剪图坐标。
/// 存在聚光（Glow）对象时：整图先暗化，再把各 Glow 区域恢复为原图亮度（聚光效果），
/// 与屏幕显示一致；其余对象照常绘制。
pub fn bake_objects(pixmap: &mut Pixmap, objects: &[Object], origin: (i32, i32)) {
    let transform = Transform::from_translate(-origin.0 as f32, -origin.1 as f32);
    if objects.iter().any(|o| matches!(o.kind, Kind::Glow { .. })) {
        // 备份原图，暗化后用于恢复 Glow 区域
        let backup = pixmap.clone();
        dim_all(pixmap, 255 - 120);
        for obj in objects {
            if let Kind::Glow { a, b } = &obj.kind {
                restore_rect(pixmap, &backup, *a, *b, origin);
            }
        }
    }
    for obj in objects {
        if matches!(obj.kind, Kind::Glow { .. }) {
            continue; // 已在上面处理，不重复绘制
        }
        super::draw_object_with(pixmap, obj, transform);
    }
}

/// 全图暗化：RGB 通道乘 `k/256`（与覆盖层暗化同系数，M3）。
fn dim_all(pixmap: &mut Pixmap, k: u32) {
    for px in pixmap.data_mut().chunks_exact_mut(4) {
        px[0] = ((px[0] as u32 * k) >> 8) as u8;
        px[1] = ((px[1] as u32 * k) >> 8) as u8;
        px[2] = ((px[2] as u32 * k) >> 8) as u8;
    }
}

/// 从备份图把 Glow 区域（对角两点，经 origin 平移）恢复到目标图。
fn restore_rect(dst: &mut Pixmap, src: &Pixmap, a: Point, b: Point, origin: (i32, i32)) {
    let (x0f, y0f) = (a.x.min(b.x), a.y.min(b.y));
    let (x1f, y1f) = (a.x.max(b.x), a.y.max(b.y));
    let (w, h) = (dst.width() as i32, dst.height() as i32);
    let x0 = (x0f as i32 - origin.0).clamp(0, w);
    let y0 = (y0f as i32 - origin.1).clamp(0, h);
    let x1 = (x1f.ceil() as i32 - origin.0).clamp(0, w);
    let y1 = (y1f.ceil() as i32 - origin.1).clamp(0, h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for row in y0..y1 {
        let off = (row as usize * dst.width() as usize + x0 as usize) * 4;
        let len = ((x1 - x0) * 4) as usize;
        dst.data_mut()[off..off + len].copy_from_slice(&src.data()[off..off + len]);
    }
}
