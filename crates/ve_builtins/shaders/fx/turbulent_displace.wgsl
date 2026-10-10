// 0 amount (px), 1 size (px), 2 complexity (octaves), 3 evolution
// (degrees; animate it to make the turbulence move).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = uv_px(i.uv) / max(params.values[1].x, 1.0);
    let e = params.values[3].x / 360.0;
    let oct = i32(clamp(params.values[2].x, 1.0, 8.0));
    let n = vec2f(fbm(p + vec2f(e * 3.0, e * 1.7), oct), fbm(p + vec2f(31.4 - e * 2.3, 7.9 + e * 2.9), oct));
    let off = (n - 0.5) * 2.0 * params.values[0].x;
    return src_clear(i.uv + off * params.scale / texels());
}
