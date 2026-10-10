// Out to white over the first half, in from white over the second.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let a = src(i.uv);
    let b = src_b(i.uv);
    let white = vec4f(vec3f(max(a.a, b.a)), max(a.a, b.a));
    if (p < 0.5) {
        return mix(a, white, p * 2.0);
    }
    return mix(white, b, p * 2.0 - 1.0);
}
