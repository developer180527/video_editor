// 0 upper left, 1 upper right, 2 lower left, 3 lower right (% of the
// frame): the picture's corners pinned there, in perspective.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p0 = pct(params.values[0].xy); // u 0, v 0
    let p1 = pct(params.values[1].xy); // u 1, v 0
    let p3 = pct(params.values[2].xy); // u 0, v 1
    let p2 = pct(params.values[3].xy); // u 1, v 1
    // The square → quad homography (Heckbert).
    let d1 = p1 - p2;
    let d2 = p3 - p2;
    let d3 = p0 - p1 + p2 - p3;
    let den = d1.x * d2.y - d2.x * d1.y;
    if (abs(den) < 1e-9) {
        return vec4f(0.0);
    }
    let g = (d3.x * d2.y - d2.x * d3.y) / den;
    let h = (d1.x * d3.y - d3.x * d1.y) / den;
    let r0 = vec3f(p1.x - p0.x + g * p1.x, p3.x - p0.x + h * p3.x, p0.x);
    let r1 = vec3f(p1.y - p0.y + g * p1.y, p3.y - p0.y + h * p3.y, p0.y);
    let r2 = vec3f(g, h, 1.0);
    let det = dot(r0, cross(r1, r2));
    if (abs(det) < 1e-9) {
        return vec4f(0.0);
    }
    // Its inverse: columns are the rows' cross products over the determinant.
    let inv = mat3x3f(cross(r1, r2), cross(r2, r0), cross(r0, r1)) * (1.0 / det);
    let w = inv * vec3f(i.uv, 1.0);
    if (w.z <= 0.0) {
        return vec4f(0.0);
    }
    return src_clear(w.xy / w.z);
}
