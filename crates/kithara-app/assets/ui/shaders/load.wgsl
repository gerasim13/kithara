// The studio's own fragment: a load bar the settings sheet draws on the GPU.
//
// `load` is the engine load the CPU cell in the top bar reads, so the field
// answers to a real measurement rather than to a constant. It is a function of
// `position` as well, so a host that mapped its pixels wrongly would show it.

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let unit = position.xy / max(kithara.viewport.xy, vec2<f32>(1.0, 1.0));
    let load = clamp(kithara.load.x, 0.0, 1.0);
    let under = step(unit.x, load);
    let grid = fract(unit.x * 24.0);
    let edge = smoothstep(0.0, 0.5, grid) * 0.35 + 0.65;
    return vec4<f32>(
        under * edge * (0.35 + load * 0.55),
        under * edge * (0.55 - load * 0.30),
        (0.18 + unit.y * 0.10) + under * edge * 0.25,
        1.0,
    );
}
