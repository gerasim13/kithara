use crate::{
    draw::{DrawList, Rect},
    hosts::layer::{HostLayer, LayerHit},
    interact::CursorShape,
    render::{WindowCommand, WindowEdge},
};

pub(crate) fn frame(bounds: Rect, thickness: f32) -> HostLayer<WindowCommand> {
    let side_width = (bounds.w - thickness * 2.0).max(0.0);
    let side_height = (bounds.h - thickness * 2.0).max(0.0);
    let east = bounds.x + bounds.w - thickness;
    let south = bounds.y + bounds.h - thickness;
    let hit =
        |area: Rect, edge| LayerHit::new(area, resize_cursor(edge), WindowCommand::Resize(edge));
    HostLayer::new(
        bounds,
        DrawList::default(),
        vec![
            hit(
                Rect {
                    h: thickness,
                    w: thickness,
                    x: bounds.x,
                    y: bounds.y,
                },
                WindowEdge::NorthWest,
            ),
            hit(
                Rect {
                    h: thickness,
                    w: side_width,
                    x: bounds.x + thickness,
                    y: bounds.y,
                },
                WindowEdge::North,
            ),
            hit(
                Rect {
                    h: thickness,
                    w: thickness,
                    x: east,
                    y: bounds.y,
                },
                WindowEdge::NorthEast,
            ),
            hit(
                Rect {
                    h: side_height,
                    w: thickness,
                    x: bounds.x,
                    y: bounds.y + thickness,
                },
                WindowEdge::West,
            ),
            hit(
                Rect {
                    h: side_height,
                    w: thickness,
                    x: east,
                    y: bounds.y + thickness,
                },
                WindowEdge::East,
            ),
            hit(
                Rect {
                    h: thickness,
                    w: thickness,
                    x: bounds.x,
                    y: south,
                },
                WindowEdge::SouthWest,
            ),
            hit(
                Rect {
                    h: thickness,
                    w: side_width,
                    x: bounds.x + thickness,
                    y: south,
                },
                WindowEdge::South,
            ),
            hit(
                Rect {
                    h: thickness,
                    w: thickness,
                    x: east,
                    y: south,
                },
                WindowEdge::SouthEast,
            ),
        ],
    )
}

pub(crate) const fn resize_cursor(edge: WindowEdge) -> CursorShape {
    match edge {
        WindowEdge::North | WindowEdge::South => CursorShape::ResizeV,
        WindowEdge::East | WindowEdge::West => CursorShape::ResizeH,
        WindowEdge::NorthWest | WindowEdge::SouthEast => CursorShape::ResizeDiagonalDown,
        WindowEdge::NorthEast | WindowEdge::SouthWest => CursorShape::ResizeDiagonalUp,
    }
}
