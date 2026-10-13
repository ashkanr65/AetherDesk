struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = positions[index] * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return output;
}

@group(0) @binding(0) var layer_sampler: sampler;
@group(0) @binding(1) var base_layer: texture_2d<f32>;
@group(0) @binding(2) var video_layer: texture_2d<f32>;
@group(0) @binding(3) var cursor_layer: texture_2d<f32>;

@fragment
fn composite(input: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(base_layer, layer_sampler, input.uv);
    let video = textureSample(video_layer, layer_sampler, input.uv);
    let cursor = textureSample(cursor_layer, layer_sampler, input.uv);
    let desktop = video + base * (1.0 - video.a);
    return cursor + desktop * (1.0 - cursor.a);
}

@group(1) @binding(0) var present_sampler: sampler;
@group(1) @binding(1) var composed_frame: texture_2d<f32>;

@fragment
fn present(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(composed_frame, present_sampler, input.uv);
}

