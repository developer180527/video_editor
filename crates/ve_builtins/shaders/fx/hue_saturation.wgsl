// 0 hue (degrees), 1 saturation (%), 2 lightness (-100..100).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    var c = encode(s.rgb);
    // Hue: a rotation about the grey axis.
    let a = radians(params.values[0].x);
    let k = vec3f(0.57735027);
    c = c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
    let l = luma(c);
    c = vec3f(l) + (c - vec3f(l)) * (params.values[1].x / 100.0);
    let light = params.values[2].x / 100.0;
    c = select(c + (vec3f(1.0) - c) * light, c * (1.0 + light), light < 0.0);
    return premul(vec4f(decode(max(c, vec3f(0.0))), s.a));
}
