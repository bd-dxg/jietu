// SPDX-License-Identifier: GPL-3.0-only
//! 滚轮缩放（M2b 性能整改）：滚动中用最近邻预览（定点映射，极快、维持帧率），
//! 静止 120ms 后由 WM_TIMER 精修为双线性/区域平均（清晰）。窗口静止时无任何重绘。

use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, KillTimer, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetTimer, SetWindowPos,
};

use super::dib::window_center;
use super::scale;
use super::window::PinWindow;

/// 防超放大内存爆炸。
pub(super) const MAX_SIDE: u32 = 4096;
const SCALE_MIN: f32 = 0.1;
const SCALE_MAX: f32 = 8.0;
/// 精修定时器 id（静止后双线性重绘）。窗口销毁后由系统自动清理。
pub(super) const TIMER_FINAL: usize = 1001;
/// 停止滚动多少毫秒后精修。
const FINALIZE_MS: usize = 120;

impl PinWindow {
    /// 滚轮缩放：更新尺寸（最近邻预览）并安排停止后的精修。
    pub(super) fn rescale_preview(&mut self, factor: f32) {
        self.scale_to(self.scale() * factor, true);
        unsafe {
            let _ = SetTimer(Some(self.hwnd()), TIMER_FINAL, FINALIZE_MS as u32, None);
        }
    }

    /// 精修：双线性/区域平均重采样（同尺寸也重绘，换质量）。
    pub(super) fn finalize_preview(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), TIMER_FINAL);
        }
        self.scale_to(self.scale(), false);
    }

    /// 重采样到目标缩放（保持窗口中心不动）。
    /// `preview` 为 true 走最近邻（快、模糊），false 走最终质量（缩小区域平均 / 放大双线性）。
    fn scale_to(&mut self, s: f32, preview: bool) {
        let s = s.clamp(SCALE_MIN, SCALE_MAX);
        let w = ((self.original.width() as f32 * s).round() as u32).clamp(1, MAX_SIDE);
        let h = ((self.original.height() as f32 * s).round() as u32).clamp(1, MAX_SIDE);
        // 预览同尺寸跳过（滚轮微旋合并到相同取整尺寸）；精修同尺寸也必须重绘（换质量）
        if preview && w == self.scaled.width() && h == self.scaled.height() {
            return;
        }
        let nw = if preview {
            scale::resample_nearest(&self.original, w, h)
        } else {
            scale::resample(&self.original, w, h)
        };
        let Some(nw) = nw else { return };
        self.scaled = nw;
        let center = unsafe { window_center(self.hwnd()) };
        unsafe {
            let _ = SetWindowPos(
                self.hwnd(),
                Some(HWND_TOPMOST),
                center.0 - w as i32 / 2,
                center.1 - h as i32 / 2,
                w as i32,
                h as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        let _ = self.rebind();
    }
}
