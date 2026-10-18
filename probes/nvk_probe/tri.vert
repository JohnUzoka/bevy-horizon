#version 450
// A rotating triangle: positions and colours come from gl_VertexIndex, so there are no buffers.
layout(push_constant) uniform Push { float angle; } push;
layout(location = 0) out vec3 v_color;

const vec2 POS[3] = vec2[](vec2(0.0, -0.6), vec2(0.6, 0.6), vec2(-0.6, 0.6));
const vec3 COL[3] = vec3[](vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0));

void main() {
    vec2 p = POS[gl_VertexIndex];
    float c = cos(push.angle), s = sin(push.angle);
    gl_Position = vec4(c * p.x - s * p.y, s * p.x + c * p.y, 0.0, 1.0);
    v_color = COL[gl_VertexIndex];
}
