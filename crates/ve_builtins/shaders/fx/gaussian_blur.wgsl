// 0 blurriness (px), 1 dimensions (both / horizontal / vertical),
// 2 repeat edge pixels.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let sigma = params.values[0].x * 0.5 * params.scale;
    let clear = !on(2u);
    switch choice(1u) {
        case 1: { return blur1(source, i.uv, vec2f(1.0, 0.0), sigma, clear); }
        case 2: { return blur1(source, i.uv, vec2f(0.0, 1.0), sigma, clear); }
        default: { return blur2(source, i.uv, sigma, clear); }
    }
}
