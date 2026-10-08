// SPDX-License-Identifier: GPL-3.0-only
//! 编辑对象模型（EDT-1/EDT-2/EDT-3/EDT-7，PRD 6.3.3）：
//! 底图保持不变，标注以矢量对象列表叠加；列表顺序即 z 序（越靠后越上层）。
//! 坐标统一为覆盖层坐标系下的物理像素。

mod curve;
mod hit;

pub use curve::{
    SEGMENTS as CURVE_SEGMENTS, dist_to_polyline, end_tangent, flatten, head_entry_t, point_at, polyline_len,
    split_left, t_at_arc_from_end,
};

/// 坐标点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 对象的外接矩形（已含线宽/箭头头部的余量）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bounds {
    fn around(x0: f32, y0: f32, x1: f32, y1: f32, pad: f32) -> Self {
        Self {
            x0: x0 - pad,
            y0: y0 - pad,
            x1: x1 + pad,
            y1: y1 + pad,
        }
    }
}

/// 对象样式（EDT-7：颜色、线宽）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// 描边/填充色（RGBA，alpha 支持矩形透明度，EDT-1）。
    pub color: [u8; 4],
    /// 线宽（物理像素）。
    pub width: f32,
}

impl Style {
    pub fn new(color: [u8; 4], width: f32) -> Self {
        Self { color, width }
    }
}

/// 对象几何（EDT-1 矩形 / EDT-2 直线箭头 / EDT-3 曲线箭头 / EDT-5 模糊 / EDT-6 高亮）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// EDT-1 矩形：对角两点；`radius` 圆角半径；`filled` 是否填充（描边始终绘制）。
    Rect {
        a: Point,
        b: Point,
        radius: f32,
        filled: bool,
    },
    /// EDT-2/EDT-3 箭头：尾部 `from` → 尖端 `to`，三次贝塞尔控制点 `c1`/`c2`。
    Arrow {
        from: Point,
        c1: Point,
        c2: Point,
        to: Point,
    },
    /// EDT-6 高亮：对角两点，荧光笔效果（半透明色块 + Multiply 混合，不遮挡文字）。
    Highlight { a: Point, b: Point },
    /// EDT-5 高斯模糊：对角两点，`radius` 为模糊半径（由工具栏强度档位映射）。
    Blur { a: Point, b: Point, radius: f32 },
}

impl Kind {
    /// 直线箭头：控制点取起点→终点的 1/3、2/3 处（EDT-3 控制柄初始位置）。
    pub fn arrow(from: Point, to: Point) -> Self {
        Kind::Arrow {
            from,
            c1: lerp(from, to, 1.0 / 3.0),
            c2: lerp(from, to, 2.0 / 3.0),
            to,
        }
    }

    /// 两个控制点（非箭头返回 None）。
    pub fn control_points(&self) -> Option<[Point; 2]> {
        match *self {
            Kind::Arrow { c1, c2, .. } => Some([c1, c2]),
            Kind::Rect { .. } | Kind::Highlight { .. } | Kind::Blur { .. } => None,
        }
    }

    /// 拖动控制柄（EDT-3）；`index` 为 0（尾部侧）或 1（头部侧）。
    pub fn set_control(&mut self, index: usize, p: Point) {
        if let Kind::Arrow { c1, c2, .. } = self {
            match index {
                0 => *c1 = p,
                _ => *c2 = p,
            }
        }
    }

    /// 双击控制柄恢复直线（EDT-3）；返回是否发生变化。
    pub fn reset_curve(&mut self) -> bool {
        let Kind::Arrow { from, to, .. } = *self else {
            return false;
        };
        let straight = Kind::arrow(from, to);
        if straight == *self {
            return false;
        }
        *self = straight;
        true
    }

    /// 几何包围盒，含线宽一半与箭头头部的余量（供脏区计算使用）。
    pub fn bounds(&self, style: &Style) -> Bounds {
        let half = style.width * 0.5 + 1.0;
        match *self {
            Kind::Rect { a, b, .. } => Bounds::around(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y), half),
            Kind::Arrow { from, c1, c2, to } => {
                // 曲线必落在控制点凸包内，故取凸包包围盒（精确上界）再补头部尺寸
                let pad = half.max(arrow_head_len(style.width) * 0.5);
                let (x0, y0, x1, y1) = curve::hull_bounds(from, c1, c2, to);
                Bounds::around(x0, y0, x1, y1, pad)
            }
            // 区域类（高亮/模糊）：整块都是绘制内容，外扩 1px 保证脏区取整
            Kind::Highlight { a, b } | Kind::Blur { a, b, .. } => {
                Bounds::around(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y), 1.0)
            }
        }
    }
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// 箭头头部长度（EDT-2：随线宽缩放，并保证最小可见尺寸）。
pub fn arrow_head_len(width: f32) -> f32 {
    (width * 4.0).max(10.0)
}

/// 一个标注对象。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Object {
    pub kind: Kind,
    pub style: Style,
}

impl Object {
    /// 一个标注对象是否命中点 `q`（容差含半个线宽，EDT-3a/EDT-7）。
    pub fn hit(&self, q: Point, tol: f32) -> bool {
        self.kind.hit(q, tol + self.style.width * 0.5)
    }

    pub fn bounds(&self) -> Bounds {
        self.kind.bounds(&self.style)
    }
}

/// 当前绘制工具（EDT-1/EDT-2/EDT-5/EDT-6）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Rect,
    Arrow,
    Highlight,
    Blur,
}

impl Tool {
    /// 工具栏顺序。
    pub const ALL: [Tool; 4] = [Tool::Rect, Tool::Arrow, Tool::Highlight, Tool::Blur];
}

/// 新建对象默认线宽（物理像素，连续可调：滚轮 / 二级工具栏）。
pub const DEFAULT_LINE_WIDTH: f32 = 4.0;

/// 模糊默认强度（EDT-5：半径，连续可调）。
pub const DEFAULT_BLUR_RADIUS: f32 = 16.0;

/// 颜色调色板（RGBA）。透明度由工具栏后续提供，当前固定 255。
pub const PALETTE: [[u8; 4]; 5] = [
    [255, 255, 255, 255],
    [229, 57, 53, 255],
    [251, 140, 0, 255],
    [67, 160, 71, 255],
    [30, 136, 229, 255],
];

/// 撤销栈深度上限，防止长时间标注累积内存。
const MAX_HISTORY: usize = 100;

/// 编辑文档：对象列表 + 快照式撤销/重做。
///
/// 快照式（每次编辑保存整份对象列表）而非逆操作命令：
/// 对象数量少（每个 40 字节左右），快照成本可忽略，且不会出现逆操作实现的隐蔽错误。
#[derive(Default)]
pub struct Document {
    objects: Vec<Object>,
    undo: Vec<Vec<Object>>,
    redo: Vec<Vec<Object>>,
    /// `begin` 记录的快照，`commit` 时归档。
    pending: Option<Vec<Object>>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    /// 可变对象列表（控制柄拖动用；不自动进历史，调用方负责 `begin`/`commit`）。
    pub fn objects_mut(&mut self) -> &mut [Object] {
        &mut self.objects
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 开始一次可撤销编辑（拖拽/新建前调用）。
    pub fn begin(&mut self) {
        self.pending = Some(self.objects.clone());
    }

    /// 结束编辑：`changed` 为真时把 `begin` 时的快照压入撤销栈。
    /// 拖拽中每帧调用 `begin` 会污染历史，调用方应在按下时 `begin`、松开时 `commit`。
    pub fn commit(&mut self, changed: bool) {
        let Some(before) = self.pending.take() else {
            return;
        };
        if !changed {
            return;
        }
        self.push_history(before);
    }

    /// 直接提交一次变更（无 `begin` 的简单操作，如添加对象后立即归档）。
    fn push_history(&mut self, before: Vec<Object>) {
        self.undo.push(before);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// 追加对象（不自动进历史，需调用方自行 `begin`/`commit`）。
    pub fn push(&mut self, obj: Object) {
        self.objects.push(obj);
    }

    /// 撤销；返回是否有变化。
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.objects, prev));
        true
    }

    /// 重做；返回是否有变化。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.objects, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(a: (f32, f32), b: (f32, f32)) -> Object {
        Object {
            kind: Kind::Rect {
                a: Point::new(a.0, a.1),
                b: Point::new(b.0, b.1),
                radius: 0.0,
                filled: false,
            },
            style: Style::new([255, 0, 0, 255], 2.0),
        }
    }

    fn arrow(a: (f32, f32), b: (f32, f32)) -> Object {
        Object {
            kind: Kind::arrow(Point::new(a.0, a.1), Point::new(b.0, b.1)),
            style: Style::new([255, 0, 0, 255], 2.0),
        }
    }

    #[test]
    fn add_undo_redo_roundtrip() {
        let mut doc = Document::new();
        doc.begin();
        doc.push(rect((0.0, 0.0), (10.0, 10.0)));
        doc.commit(true);

        assert_eq!(doc.objects().len(), 1);
        assert!(doc.can_undo());

        assert!(doc.undo());
        assert!(doc.objects().is_empty());
        assert!(doc.can_redo());

        assert!(doc.redo());
        assert_eq!(doc.objects().len(), 1);
    }

    #[test]
    fn commit_false_keeps_history_clean() {
        let mut doc = Document::new();
        doc.begin();
        doc.push(rect((0.0, 0.0), (1.0, 1.0)));
        doc.commit(false); // 误触，丢弃

        assert!(!doc.can_undo());
        assert_eq!(doc.objects().len(), 1);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::new();
        doc.begin();
        doc.push(rect((0.0, 0.0), (5.0, 5.0)));
        doc.commit(true);
        doc.undo();
        assert!(doc.can_redo());

        doc.begin();
        doc.push(arrow((0.0, 0.0), (9.0, 9.0)));
        doc.commit(true);
        assert!(!doc.can_redo());

        assert!(doc.undo());
        assert!(doc.objects().is_empty());
        assert!(doc.redo());
        assert!(matches!(doc.objects()[0].kind, Kind::Arrow { .. }));
    }

    #[test]
    fn undoing_past_empty_is_noop() {
        let mut doc = Document::new();
        assert!(!doc.undo());
        assert!(!doc.redo());
    }

    #[test]
    fn history_is_bounded() {
        let mut doc = Document::new();
        for i in 0..(MAX_HISTORY + 20) {
            doc.begin();
            doc.push(rect((i as f32, 0.0), (i as f32 + 1.0, 1.0)));
            doc.commit(true);
        }
        assert!(doc.undo.len() <= MAX_HISTORY);
    }

    #[test]
    fn bounds_include_stroke_and_arrow_head() {
        let thin = rect((10.0, 20.0), (30.0, 40.0));
        let b = thin.bounds();
        assert!(b.x0 < 10.0 && b.y0 < 20.0 && b.x1 > 30.0 && b.y1 > 40.0);

        // 水平箭头：头部沿 x 方向外扩
        let a = arrow((0.0, 0.0), (100.0, 0.0));
        let ab = a.bounds();
        assert!(ab.x1 > 100.0, "箭头头部应超出尖端：{ab:?}");
        assert!(ab.x0 <= 0.0);
    }

    #[test]
    fn new_arrow_is_straight_with_third_point_handles() {
        let a = arrow((0.0, 0.0), (90.0, 0.0));
        let [c1, c2] = a.kind.control_points().unwrap();
        assert!(
            (c1.x - 30.0).abs() < 1e-3 && (c2.x - 60.0).abs() < 1e-3,
            "控制柄应在 1/3、2/3 处"
        );
        // 直线：中点在两点连线上
        let mid = point_at(Point::new(0.0, 0.0), c1, c2, Point::new(90.0, 0.0), 0.5);
        assert!((mid.x - 45.0).abs() < 0.01 && mid.y.abs() < 0.01);
    }

    #[test]
    fn control_drag_bends_then_reset_restores_straight() {
        let mut obj = arrow((0.0, 0.0), (90.0, 0.0));
        obj.kind.set_control(0, Point::new(30.0, 60.0));
        let [c1, _] = obj.kind.control_points().unwrap();
        assert_eq!(c1, Point::new(30.0, 60.0));
        // 弯曲后包围盒应向上扩张
        assert!(obj.bounds().y1 > 40.0);

        obj.kind.reset_curve();
        let [c1, c2] = obj.kind.control_points().unwrap();
        assert!((c1.x - 30.0).abs() < 1e-3 && c1.y.abs() < 1e-3);
        assert!((c2.x - 60.0).abs() < 1e-3 && c2.y.abs() < 1e-3);
    }

    #[test]
    fn hit_test_rect_and_arrow() {
        let r = rect((10.0, 10.0), (60.0, 60.0));
        assert!(r.hit(Point::new(10.0, 30.0), 3.0), "左边框应命中");
        assert!(!r.hit(Point::new(35.0, 35.0), 3.0), "未填充矩形内部不命中");

        let filled = Object {
            kind: Kind::Rect {
                a: Point::new(10.0, 10.0),
                b: Point::new(60.0, 60.0),
                radius: 0.0,
                filled: true,
            },
            style: Style::new([0, 0, 0, 255], 2.0),
        };
        assert!(filled.hit(Point::new(35.0, 35.0), 3.0), "填充矩形内部应命中");

        // 弯曲箭头：曲线经过 (45, 45) 附近，而非直线中点
        let mut a = arrow((0.0, 0.0), (90.0, 0.0));
        a.kind.set_control(0, Point::new(0.0, 60.0));
        a.kind.set_control(1, Point::new(90.0, 60.0));
        let on_curve = point_at(
            Point::new(0.0, 0.0),
            Point::new(0.0, 60.0),
            Point::new(90.0, 60.0),
            Point::new(90.0, 0.0),
            0.5,
        );
        assert!(a.hit(on_curve, 3.0), "曲线上的点应命中：{on_curve:?}");
        assert!(!a.hit(Point::new(45.0, 0.0), 3.0), "直线位置不应命中弯曲箭头");
    }

    #[test]
    fn area_objects_hit_and_bounds() {
        let hl = Object {
            kind: Kind::Highlight {
                a: Point::new(10.0, 10.0),
                b: Point::new(50.0, 40.0),
            },
            style: Style::new([255, 240, 0, 255], 2.0),
        };
        assert!(hl.hit(Point::new(30.0, 25.0), 3.0), "高亮内部应命中");
        assert!(!hl.hit(Point::new(60.0, 25.0), 3.0), "高亮外部不应命中");
        let b = hl.bounds();
        assert!(b.x0 <= 10.0 && b.x1 >= 50.0 && b.y0 <= 10.0 && b.y1 >= 40.0);

        let blur = Object {
            kind: Kind::Blur {
                a: Point::new(1.0, 2.0),
                b: Point::new(11.0, 22.0),
                radius: 16.0,
            },
            style: Style::new([0, 0, 0, 255], 2.0),
        };
        assert!(blur.hit(Point::new(5.0, 10.0), 3.0), "模糊内部应命中");
        assert!(!blur.hit(Point::new(30.0, 30.0), 3.0), "模糊外部不应命中");
        assert!(blur.kind.control_points().is_none(), "区域类无控制柄");
        assert!(Tool::ALL.contains(&Tool::Highlight) && Tool::ALL.contains(&Tool::Blur));
    }
}
