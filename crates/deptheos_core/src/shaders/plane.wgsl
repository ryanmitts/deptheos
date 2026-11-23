// A frame's weight in the depth blend at depth-map value `z`.

#define_import_path deptheos::plane

// A linear ramp to each neighboring stacked frame's plane (`PlaneRamp`), so
// weights sum to one across a left-out frame. `PlaneRamp::weight`
// (plane.rs) is the CPU copy.
fn plane_weight(z: f32, plane: f32, below: f32, above: f32) -> f32 {
    let d = z - plane;
    let span = select(above, below, d < 0.0);
    return max(0.0, 1.0 - abs(d) / max(span, 0.00001));
}
