// 0 wave type (sine / square / triangle / sawtooth), 1 height (px),
// 2 width (px), 3 direction (degrees), 4 speed (cycles a second), 5 phase.
fn wave(kind: i32, s: f32) -> f32 {
    switch kind {
        case 1: { return select(-1.0, 1.0, fract(s) < 0.5); }
        case 2: { return 1.0 - 4.0 * abs(fract(s + 0.25) - 0.5); }
        case 3: { return 2.0 * fract(s) - 1.0; }
        default: { return sin(TAU * s); }
    }
}

@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let d = heading(params.values[3].x);
    let p = uv_px(i.uv);
    let s = dot(p, d) / max(params.values[2].x, 1.0) - params.values[4].x * params.time + params.values[5].x / 360.0;
    let n = vec2f(-d.y, d.x);
    let off = n * params.values[1].x * wave(choice(0u), s);
    return src_clear(i.uv + off * params.scale / texels());
}
