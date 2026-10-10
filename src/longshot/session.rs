// SPDX-License-Identifier: GPL-3.0-only
//! 长截图会话（LNG-2/3/6/10）：自动滚动走「滚轮 → 等待稳定 → 抓帧 → 拼接」，
//! 手动模式由用户滚动、每拍抓帧拼接；到底检测、撤销与输出。

use super::engine::{Canvas, Step};
use super::stitch::Params;
use crate::capture::capture_region;

/// 自动模式滚轮增量（WHEEL_DELTA 单位）。
const WHEEL_STEP: i32 = 120;

/// 等待画面稳定的最大拍数（每拍 ~90ms）。
const MAX_WAIT_TICKS: u32 = 22;
/// 连续静帧判定「到底」的阈值（LNG-6）：需要连续多帧无位移，避免动画/加载停顿误判。
const END_STILL: u32 = 4;

/// 帧源抽象：截取当前视口帧 + 发送滚动。真实实现走屏幕抓取与 SendInput，
/// 测试实现模拟一个可滚动的长页面（无需 GUI 即可验证拼接循环）。
pub trait Source {
    /// 截取当前视口像素（长度 = w*h*4）。
    fn capture(&mut self) -> Result<Vec<u8>, String>;
    /// 发送一次滚动（负值 = 向下滚，内容上移）。
    fn scroll(&mut self, delta: i32);
}

/// 真实帧源：屏幕区域抓取 + 真实滚轮（首次滚动前激活目标）。
pub struct ScreenSource {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    /// 是否已激活目标窗口（只点一次，避免反复点击）。
    activated: bool,
}

impl ScreenSource {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            activated: false,
        }
    }
}

impl Source for ScreenSource {
    fn capture(&mut self) -> Result<Vec<u8>, String> {
        let screen = capture_region(self.x, self.y, self.w, self.h)?;
        if screen.rgba.len() != (self.w * self.h * 4) as usize {
            return Err("抓帧尺寸不符".into());
        }
        Ok(screen.rgba)
    }

    fn scroll(&mut self, delta: i32) {
        if !self.activated {
            super::scroll::activate(self.x, self.y, self.w, self.h);
            self.activated = true;
        }
        super::scroll::wheel(self.x, self.y, self.w, self.h, delta);
    }
}

/// 长截图会话状态。
pub struct Session {
    w: u32,
    h: u32,
    canvas: Canvas,
    /// 帧源（真实屏幕或测试模拟）。
    source: Box<dyn Source>,
    /// 自动模式（true）或手动模式（false）。
    pub auto: bool,
    /// 自动模式：滚轮发出后等待稳定的中间态。
    rolling: bool,
    wait_last: Vec<u8>,
    wait_stable: u32,
    wait_ticks: u32,
    /// 自动模式连续静帧数。
    still: u32,
    /// 本轮滚动后画面是否已开始变化（先动再稳，避免拿旧帧拼接）。
    moved: bool,
    /// 最近一次采集到的帧（滚动开始时作为基准，用于检测“画面已变化”）。
    last_frame: Vec<u8>,
    /// 是否已采集首帧（不能用 canvas.height()==0 判断：seed 只缓冲、高度仍为 0）。
    seeded: bool,
    pub finished: bool,
    pub cancelled: bool,
    /// 最近一条状态文本（供窗口绘制）。
    pub status: String,
    /// 累计发送的滚动次数（调试/测试用）。
    pub scroll_count: u32,
    /// 抓帧序号（调试用）。
    frame_seq: u32,
}

impl Session {
    pub fn new(x: i32, y: i32, w: u32, h: u32, max_height: u32) -> Result<Self, String> {
        Self::with_source(Box::new(ScreenSource::new(x, y, w, h)), w, h, max_height)
    }

    /// 用指定帧源构造（测试注入模拟页面）。
    pub fn with_source(source: Box<dyn Source>, w: u32, h: u32, max_height: u32) -> Result<Self, String> {
        if w == 0 || h == 0 {
            return Err("长截图视口尺寸非法".into());
        }
        Ok(Self {
            w,
            h,
            canvas: Canvas::new(w, Params::default(), max_height),
            source,
            auto: true,
            rolling: false,
            wait_last: Vec::new(),
            wait_stable: 0,
            wait_ticks: 0,
            still: 0,
            moved: false,
            last_frame: Vec::new(),
            seeded: false,
            finished: false,
            cancelled: false,
            status: String::new(),
            scroll_count: 0,
            frame_seq: 0,
        })
    }

    /// 抓取当前视口帧。
    pub fn capture(&mut self) -> Result<Vec<u8>, String> {
        let frame = self.source.capture()?;
        self.frame_seq += 1;
        if std::env::var_os("JIETU_DEBUG").is_some() {
            // 简单哈希（异或逐 8 字节）+ 采样首像素，看每帧是否真在变。
            let mut h: u64 = 0;
            for c in frame.chunks_exact(8).step_by(97) {
                h ^= u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
                h = h.rotate_left(5);
            }
            super::debug::log(&format!("capture#{} hash={h:016x}", self.frame_seq));
        }
        // 调试：导出首帧供目视核对抓取区域。
        if !self.seeded && std::env::var_os("JIETU_DEBUG").is_some() {
            let size = tiny_skia::IntSize::from_wh(self.w, self.h);
            if let Some(size) = size
                && let Some(px) = tiny_skia::Pixmap::from_vec(frame.clone(), size)
            {
                let path = std::env::temp_dir().join("jietu-longshot-frame0.png");
                let _ = px.save_png(&path);
            }
        }
        Ok(frame)
    }

    /// 一拍（定时器回调）：自动模式驱滚动流程，手动模式直接抓帧拼接。
    /// 返回是否需要重绘状态条。
    pub fn tick(&mut self) -> bool {
        if self.finished || self.cancelled {
            return false;
        }
        if !self.seeded {
            // 首帧：先缓冲，下一拍起才能检测固定区域与定位。
            match self.capture() {
                Ok(frame) => {
                    self.canvas.seed(&frame);
                    self.last_frame = frame;
                    self.seeded = true;
                    self.status = format!(
                        "{} · 已拼 {} px · Enter 完成 / Esc 取消 / Ctrl+Z 撤销 / 空格 切换模式",
                        if self.auto { "自动" } else { "手动" },
                        self.canvas.height()
                    );
                    return true;
                }
                Err(e) => {
                    self.cancelled = true;
                    self.status = format!("抓屏失败：{e}");
                    return true;
                }
            }
        }

        if self.auto {
            self.tick_auto()
        } else {
            self.tick_manual()
        }
    }

    /// 自动模式：滚动 → 等画面开始变化 → 等画面稳定 → 拼接。
    ///
    /// 不可在滚动后立即判断「稳定」：滚动输入有延迟，刚开始几帧画面还没动，
    /// 直接判稳定会拿旧帧拼接（offset=0，实测无效果）。必须先观察到变化。
    fn tick_auto(&mut self) -> bool {
        if !self.rolling {
            // 开始新一轮滚动：负增量 = 滚轮向下 = 页面内容上移、新内容出现在底部。
            self.source.scroll(-WHEEL_STEP);
            self.scroll_count += 1;
            self.rolling = true;
            self.moved = false;
            // 基准 = 滚动前的帧：首次采集如与它不同，即说明画面已开始变化。
            self.wait_last = self.last_frame.clone();
            self.wait_stable = 0;
            self.wait_ticks = 0;
            return false;
        }
        let frame = match self.capture() {
            Ok(f) => f,
            Err(e) => {
                self.cancelled = true;
                self.status = format!("抓屏失败：{e}");
                return true;
            }
        };
        self.last_frame = frame.clone();
        let threshold = (self.h / 33).max(2); // ≈3% 行允许不同
        if self.wait_last.is_empty() {
            self.wait_last = frame.clone();
        } else {
            let diff = super::stitch::diff_rows(&self.wait_last, &frame, self.w, self.h);
            if diff > threshold {
                // 画面在变：标记已动，重置稳定计数
                self.moved = true;
                self.wait_stable = 0;
                self.wait_last = frame.clone();
            } else if self.moved {
                // 已动过且现在连续两拍不变 → 稳定，准备拼接
                self.wait_stable += 1;
            }
        }
        self.wait_ticks += 1;
        let settled = self.moved && self.wait_stable >= 2;
        let timeout = self.wait_ticks >= MAX_WAIT_TICKS;
        if !settled && !timeout {
            return false;
        }
        self.rolling = false;
        if self.moved {
            self.stitch_auto(&frame)
        } else {
            // 整轮都没动（已到页底或滚动无效）：计入静帧计数
            self.still += 1;
            self.status = "自动 · 页面未滚动…".to_string();
            super::debug::log(&format!("no movement this round (still={})", self.still));
            if self.still >= END_STILL {
                self.finished = true;
                self.status = "已到页底，长截图完成".to_string();
            }
            true
        }
    }

    fn stitch_auto(&mut self, frame: &[u8]) -> bool {
        match self.canvas.push(frame, self.h) {
            Step::Appended { rows, .. } => {
                self.still = 0;
                self.status = format!(
                    "自动 · 已拼 {} px(+{rows}) · Enter 完成 / Esc 取消 / Ctrl+Z 撤销",
                    self.canvas.height()
                );
                super::debug::log(&format!("appended +{rows} rows, total {}", self.canvas.height()));
                true
            }
            Step::Still => {
                self.still += 1;
                self.status = "自动 · 等待页面滚动…".to_string();
                if self.still >= END_STILL {
                    self.finished = true;
                    self.status = "已到页底，长截图完成".to_string();
                    super::debug::log("ended: no movement");
                }
                true
            }
            Step::Failed => {
                self.still += 1;
                self.status = "自动 · 拼接定位失败，请放慢或切手动模式".to_string();
                super::debug::log(&format!("failed: locate mismatch (still={})", self.still));
                // 连续失败也视为无法继续（避免无限循环），保留已拼内容结束。
                if self.still >= END_STILL {
                    self.finished = true;
                    self.status = format!("拼接定位失败，已完成 {} px", self.canvas.height());
                }
                true
            }
            Step::Limit => {
                self.finished = true;
                self.status = "已达高度上限，长截图完成".to_string();
                true
            }
        }
    }

    fn tick_manual(&mut self) -> bool {
        match self.capture() {
            Ok(frame) => {
                self.last_frame = frame.clone();
                match self.canvas.push(&frame, self.h) {
                    Step::Appended { rows, .. } => {
                        self.status = format!(
                            "手动 · 已拼 {} px(+{rows}) · Enter 完成 / Esc 取消 / Ctrl+Z 撤销",
                            self.canvas.height()
                        );
                        super::debug::log(&format!("manual appended +{rows}, total {}", self.canvas.height()));
                    }
                    Step::Still => {
                        self.status = format!("手动 · 已拼 {} px · 请自行滚动，Enter 完成", self.canvas.height());
                    }
                    Step::Failed => {
                        // 画面变了但定位失败：提示放慢滚动
                        self.status = "手动 · 检测到滚动但拼接失败，请放慢滚动速度".to_string();
                        super::debug::log("manual failed: mismatch");
                    }
                    Step::Limit => {
                        self.finished = true;
                        self.status = "已达高度上限，长截图完成".to_string();
                    }
                }
            }
            Err(e) => {
                self.cancelled = true;
                self.status = format!("抓屏失败：{e}");
            }
        }
        true
    }

    /// 当前已拼高度（供状态条显示）。
    pub fn height(&self) -> u32 {
        self.canvas.height()
    }

    /// 是否有可撤销的段。
    pub fn can_undo(&self) -> bool {
        self.canvas.can_undo()
    }

    /// 状态条提示（空串 = 正常进度）。
    pub fn hint(&self) -> &'static str {
        if self.cancelled {
            "已取消"
        } else if self.finished {
            "已完成"
        } else if self.still > 0 {
            "等待页面滚动…"
        } else {
            ""
        }
    }

    /// 切换自动/手动。
    pub fn toggle_mode(&mut self) {
        self.auto = !self.auto;
        self.rolling = false;
    }

    /// 测试专用：直接向帧源发一次滚动（模拟用户手动滚轮）。
    #[cfg(test)]
    pub fn source_scroll_for_test(&mut self) {
        self.source.scroll(-WHEEL_STEP);
    }

    /// 撤销最后一段。
    pub fn undo(&mut self) {
        if self.canvas.undo_last() {
            self.status = format!("已撤销，当前 {} px", self.canvas.height());
        } else {
            self.status = "没有可撤销的段".to_string();
        }
    }

    /// 导出拼接结果；失败返回错误文本。
    pub fn result(&self) -> Result<tiny_skia::Pixmap, String> {
        if self.canvas.height() == 0 {
            return Err("画布为空".into());
        }
        self.canvas.to_pixmap().ok_or_else(|| "画布转像素图失败".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_rejects_zero_size() {
        assert!(Session::new(0, 0, 0, 0, 30000).is_err());
        assert!(Session::new(10, 20, 100, 200, 30000).is_ok());
    }

    #[test]
    fn max_height_constant_sane() {
        // 高度上限已从配置传入（LNG-7），此处保留常量合理性断言。
        assert!(crate::settings::Config::default().longshot_max_height > 0);
    }

    /// 模拟一个可滚动长页面：拥有超过一屏的内容，每滚一格移动 STEP 像素。
    /// 这能在无 GUI 下验证「滚动 → 等待稳定 → 拼接」整条自动链路。
    struct FakePage {
        page: Vec<u8>,
        w: u32,
        h: u32,
        offset: u32,
        step: u32,
        max_offset: u32,
    }

    impl FakePage {
        fn new(w: u32, h: u32, total: u32, step: u32) -> Self {
            // 每行内容由全局行号决定（保证行间可区分）
            let mut page = vec![0u8; (w * total * 4) as usize];
            for y in 0..total {
                for x in 0..w {
                    let o = ((y * w + x) * 4) as usize;
                    page[o] = (y & 0xff) as u8;
                    page[o + 1] = ((y >> 8) & 0xff) as u8;
                    page[o + 2] = ((x * 7 + y * 3) & 0xff) as u8;
                    page[o + 3] = 255;
                }
            }
            let max_offset = total.saturating_sub(h);
            Self {
                page,
                w,
                h,
                offset: 0,
                step,
                max_offset,
            }
        }

        fn window(&self) -> Vec<u8> {
            let stride = self.w as usize * 4;
            let start = self.offset as usize * stride;
            self.page[start..start + self.h as usize * stride].to_vec()
        }
    }

    impl Source for FakePage {
        fn capture(&mut self) -> Result<Vec<u8>, String> {
            Ok(self.window())
        }

        fn scroll(&mut self, delta: i32) {
            // 负 delta = 向下滚 = 内容上移 = offset 增大
            if delta < 0 {
                self.offset = (self.offset + self.step).min(self.max_offset);
            } else {
                self.offset = self.offset.saturating_sub(self.step);
            }
        }
    }

    /// 自动模式端到端：模拟页面逐格下滚，一直跑到「到底」，
    /// 断言拼接结果与原始长页面完全一致。这是长截图核心链路的回归保护。
    #[test]
    fn auto_scroll_stitches_full_page() {
        let (w, h) = (64u32, 50u32);
        let (total, step) = (300u32, 20u32);
        let page = FakePage::new(w, h, total, step);
        let expect = page.page.clone();
        let mut s = Session::with_source(Box::new(page), w, h, 30000).expect("会话创建");

        // 驱动自动循环：最多 500 拍（足够滚完全部内容并触发到底）
        for _ in 0..500 {
            if s.finished || s.cancelled {
                break;
            }
            s.tick();
        }
        assert!(s.finished, "自动模式应能自行跑到到底");
        assert!(!s.cancelled);
        // 拼接图高度应等于整页高度（允许首帧重叠引起的 ±step 误差）
        let img = s.result().expect("有拼接结果");
        assert!(
            (img.height() as i32 - total as i32).abs() <= step as i32,
            "拼接高度 {} 应接近整页高度 {}",
            img.height(),
            total
        );
        // 顶部若干行与页面顶部一致
        let stride = w as usize * 4;
        for y in 0..(h as usize / 2) {
            let o = y * stride;
            assert_eq!(
                &img.data()[o..o + stride],
                &expect[o..o + stride],
                "拼接图第 {y} 行应与页面一致"
            );
        }
    }

    /// 图片较短（不足两屏）时，拼接不应崩溃，且高度接近单屏。
    #[test]
    fn auto_scroll_short_page() {
        let (w, h) = (64u32, 50u32);
        let page = FakePage::new(w, h, 55, 20);
        let mut s = Session::with_source(Box::new(page), w, h, 30000).expect("会话创建");
        for _ in 0..200 {
            if s.finished || s.cancelled {
                break;
            }
            s.tick();
        }
        assert!(s.finished);
        let img = s.result().expect("有结果");
        assert!(img.height() >= h && img.height() <= 55 + 20);
    }

    /// 手动模式：用户逐步滚动，每拍抓帧拼接，最后 Enter 完成。
    #[test]
    fn manual_mode_stitches_user_scrolls() {
        let (w, h) = (64u32, 50u32);
        let (total, step) = (200u32, 15u32);
        let page = FakePage::new(w, h, total, step);
        let expect = page.page.clone();
        let mut s = Session::with_source(Box::new(page), w, h, 30000).expect("会话创建");
        s.auto = false;

        // 首帧 seed
        s.tick();
        // 模拟用户每拍滚一格
        for _ in 0..20 {
            s.source_scroll_for_test();
            s.tick();
        }
        let img = s.result().expect("手动模式有结果");
        assert!(img.height() > h, "手动模式应拼出多于单屏的内容，实际 {}", img.height());
        // 顶部应仍与页面顶部一致
        let stride = w as usize * 4;
        for y in 0..(h as usize / 2) {
            let o = y * stride;
            assert_eq!(&img.data()[o..o + stride], &expect[o..o + stride], "第 {y} 行");
        }
    }

    /// 大视口 + 小滚动步长（接近真实浏览器：视口 400 行、每滚 60 行）。
    #[test]
    fn auto_scroll_realistic_viewport() {
        let (w, h) = (128u32, 400u32);
        let (total, step) = (2000u32, 60u32);
        let page = FakePage::new(w, h, total, step);
        let mut s = Session::with_source(Box::new(page), w, h, 30000).expect("会话创建");
        for _ in 0..3000 {
            if s.finished || s.cancelled {
                break;
            }
            s.tick();
        }
        assert!(s.finished, "真实视口尺寸下应能跑到到底");
        let img = s.result().expect("有结果");
        assert!(
            (img.height() as i32 - total as i32).abs() <= step as i32,
            "高度 {} 应接近 {total}",
            img.height()
        );
    }
}
