// 0 squares across, 1 softness (%): one set of squares wipes in, then
// the other.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let n = max(round(params.values[0].x), 1.0);
    let g = vec2f(i.uv.x * n, i.uv.y * n / aspect());
    let cell = floor(g);
    let odd = (i32(cell.x) + i32(cell.y)) & 1;
    let f = (fract(g.x) + f32(odd)) * 0.5;
    return mix(src(i.uv), src_b(i.uv), reveal(f, params.progress, params.values[1].x / 100.0));
}
