// 0 input black, 1 input white, 2 gamma, 3 output black, 4 output white
// (levels 0..255 of the display-encoded picture).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let ib = params.values[0].x / 255.0;
    let iw = params.values[1].x / 255.0;
    let g = max(params.values[2].x, 0.01);
    let ob = params.values[3].x / 255.0;
    let ow = params.values[4].x / 255.0;
    var x = (encode(s.rgb) - ib) / max(iw - ib, 1e-4);
    x = pow(max(x, vec3f(0.0)), vec3f(1.0 / g));
    let y = ob + x * (ow - ob);
    return premul(vec4f(decode(max(y, vec3f(0.0))), s.a));
}
