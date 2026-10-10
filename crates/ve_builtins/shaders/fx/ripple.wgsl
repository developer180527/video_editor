// 0 centre (%), 1 amplitude (px), 2 wavelength (px), 3 speed (cycles a
// second), 4 damping (%): rings moving out from the centre.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[0].xy);
    let d = uv_px(i.uv) - uv_px(c);
    let r = length(d);
    if (r < 1e-3) {
        return src_clear(i.uv);
    }
    let s = r / max(params.values[2].x, 1.0) - params.values[3].x * params.time;
    let fall = exp(-params.values[4].x / 100.0 * 4.0 * r / max(uv_px(vec2f(1.0)).y, 1.0));
    let off = d / r * params.values[1].x * sin(TAU * s) * fall;
    return src_clear(i.uv + off * params.scale / texels());
}
