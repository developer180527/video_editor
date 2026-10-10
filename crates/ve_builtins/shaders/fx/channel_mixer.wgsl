// 0..8: each output channel's share of red, green and blue (%), in linear
// light; 9 monochrome (the red output everywhere).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = src(i.uv);
    let v = params.values;
    let r = dot(p.rgb, vec3f(v[0].x, v[1].x, v[2].x)) / 100.0;
    let g = dot(p.rgb, vec3f(v[3].x, v[4].x, v[5].x)) / 100.0;
    let b = dot(p.rgb, vec3f(v[6].x, v[7].x, v[8].x)) / 100.0;
    var c = vec3f(r, g, b);
    if (v[9].x > 0.5) {
        c = vec3f(r);
    }
    return vec4f(c, p.a);
}
