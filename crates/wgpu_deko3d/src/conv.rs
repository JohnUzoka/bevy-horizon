//! wgpu ↔ deko3d enum and format conversions.
use wgpu_hal as hal;
use wgpu_types as wgt;

use super::dk::*;

/// deko3d image format and whether it can be a color render target.
pub fn texture_format(format: wgt::TextureFormat) -> Option<(u32, bool)> {
    use wgt::TextureFormat as F;
    Some(match format {
        F::R8Unorm => (DkImageFormat_R8_Unorm, true),
        F::R8Snorm => (DkImageFormat_R8_Snorm, false),
        F::R8Uint => (DkImageFormat_R8_Uint, true),
        F::R8Sint => (DkImageFormat_R8_Sint, true),
        F::R16Uint => (DkImageFormat_R16_Uint, true),
        F::R16Sint => (DkImageFormat_R16_Sint, true),
        F::R16Unorm => (DkImageFormat_R16_Unorm, true),
        F::R16Snorm => (DkImageFormat_R16_Snorm, false),
        F::R16Float => (DkImageFormat_R16_Float, true),
        F::Rg8Unorm => (DkImageFormat_RG8_Unorm, true),
        F::Rg8Snorm => (DkImageFormat_RG8_Snorm, false),
        F::Rg8Uint => (DkImageFormat_RG8_Uint, true),
        F::Rg8Sint => (DkImageFormat_RG8_Sint, true),
        F::R32Uint => (DkImageFormat_R32_Uint, true),
        F::R32Sint => (DkImageFormat_R32_Sint, true),
        F::R32Float => (DkImageFormat_R32_Float, true),
        F::Rg16Uint => (DkImageFormat_RG16_Uint, true),
        F::Rg16Sint => (DkImageFormat_RG16_Sint, true),
        F::Rg16Unorm => (DkImageFormat_RG16_Unorm, true),
        F::Rg16Snorm => (DkImageFormat_RG16_Snorm, false),
        F::Rg16Float => (DkImageFormat_RG16_Float, true),
        F::Rgba8Unorm => (DkImageFormat_RGBA8_Unorm, true),
        F::Rgba8UnormSrgb => (DkImageFormat_RGBA8_Unorm_sRGB, true),
        F::Rgba8Snorm => (DkImageFormat_RGBA8_Snorm, false),
        F::Rgba8Uint => (DkImageFormat_RGBA8_Uint, true),
        F::Rgba8Sint => (DkImageFormat_RGBA8_Sint, true),
        F::Bgra8Unorm => (DkImageFormat_BGRA8_Unorm, true),
        F::Bgra8UnormSrgb => (DkImageFormat_BGRA8_Unorm_sRGB, true),
        F::Rgb9e5Ufloat => (DkImageFormat_E5BGR9_Float, false),
        F::Rgb10a2Uint => (DkImageFormat_RGB10A2_Uint, true),
        F::Rgb10a2Unorm => (DkImageFormat_RGB10A2_Unorm, true),
        F::Rg11b10Ufloat => (DkImageFormat_RG11B10_Float, true),
        F::Rg32Uint => (DkImageFormat_RG32_Uint, true),
        F::Rg32Sint => (DkImageFormat_RG32_Sint, true),
        F::Rg32Float => (DkImageFormat_RG32_Float, true),
        F::Rgba16Uint => (DkImageFormat_RGBA16_Uint, true),
        F::Rgba16Sint => (DkImageFormat_RGBA16_Sint, true),
        F::Rgba16Unorm => (DkImageFormat_RGBA16_Unorm, true),
        F::Rgba16Snorm => (DkImageFormat_RGBA16_Snorm, false),
        F::Rgba16Float => (DkImageFormat_RGBA16_Float, true),
        F::Rgba32Uint => (DkImageFormat_RGBA32_Uint, true),
        F::Rgba32Sint => (DkImageFormat_RGBA32_Sint, true),
        F::Rgba32Float => (DkImageFormat_RGBA32_Float, true),
        F::Stencil8 => (DkImageFormat_S8, false),
        F::Depth16Unorm => (DkImageFormat_Z16, false),
        F::Depth24Plus => (DkImageFormat_Z24X8, false),
        F::Depth24PlusStencil8 => (DkImageFormat_Z24S8, false),
        F::Depth32Float => (DkImageFormat_ZF32, false),
        F::Depth32FloatStencil8 => (DkImageFormat_ZF32_X24S8, false),
        F::Bc1RgbaUnorm => (DkImageFormat_RGBA_BC1, false),
        F::Bc1RgbaUnormSrgb => (DkImageFormat_RGBA_BC1_sRGB, false),
        F::Bc2RgbaUnorm => (DkImageFormat_RGBA_BC2, false),
        F::Bc2RgbaUnormSrgb => (DkImageFormat_RGBA_BC2_sRGB, false),
        F::Bc3RgbaUnorm => (DkImageFormat_RGBA_BC3, false),
        F::Bc3RgbaUnormSrgb => (DkImageFormat_RGBA_BC3_sRGB, false),
        F::Bc4RUnorm => (DkImageFormat_R_BC4_Unorm, false),
        F::Bc4RSnorm => (DkImageFormat_R_BC4_Snorm, false),
        F::Bc5RgUnorm => (DkImageFormat_RG_BC5_Unorm, false),
        F::Bc5RgSnorm => (DkImageFormat_RG_BC5_Snorm, false),
        F::Bc6hRgbUfloat => (DkImageFormat_RGBA_BC6H_UF16_Float, false),
        F::Bc6hRgbFloat => (DkImageFormat_RGBA_BC6H_SF16_Float, false),
        F::Bc7RgbaUnorm => (DkImageFormat_RGBA_BC7_Unorm, false),
        F::Bc7RgbaUnormSrgb => (DkImageFormat_RGBA_BC7_Unorm_sRGB, false),
        _ => return None,
    })
}

pub fn format_capabilities(format: wgt::TextureFormat) -> hal::TextureFormatCapabilities {
    use hal::TextureFormatCapabilities as Cap;
    let Some((dk_format, renderable)) = texture_format(format) else {
        return Cap::empty();
    };
    let traits = FORMAT_TRAIT_FLAGS[dk_format as usize];
    let multisample = Cap::MULTISAMPLE_X2 | Cap::MULTISAMPLE_X4 | Cap::MULTISAMPLE_X8;
    let mut caps = Cap::SAMPLED | Cap::COPY_SRC | Cap::COPY_DST;
    if format.is_depth_stencil_format() {
        return caps | Cap::DEPTH_STENCIL_ATTACHMENT | multisample;
    }
    let float = matches!(format.sample_type(None, None), Some(wgt::TextureSampleType::Float { .. }));
    if matches!(format.sample_type(None, None), Some(wgt::TextureSampleType::Float { filterable: true })) {
        caps |= Cap::SAMPLED_LINEAR;
    }
    if renderable {
        caps |= Cap::COLOR_ATTACHMENT | multisample;
        if float {
            caps |= Cap::COLOR_ATTACHMENT_BLEND;
        }
        // deko3d resolves on the 2D engine.
        if traits & FormatTraitFlags_CanUse2DEngine != 0 && float {
            caps |= Cap::MULTISAMPLE_RESOLVE;
        }
    }
    if traits & FormatTraitFlags_CanLoadStore != 0 && !format.is_srgb() && !format.is_compressed() {
        caps |= Cap::STORAGE_READ_ONLY | Cap::STORAGE_WRITE_ONLY | Cap::STORAGE_READ_WRITE;
    }
    caps
}

/// How deko3d reads a render pass clear value for this format.
pub fn clear_bits(format: wgt::TextureFormat, color: wgt::Color) -> [u32; 4] {
    let c = [color.r, color.g, color.b, color.a];
    match format.sample_type(None, None) {
        Some(wgt::TextureSampleType::Uint) => c.map(|v| v as u32),
        Some(wgt::TextureSampleType::Sint) => c.map(|v| v as i32 as u32),
        _ => c.map(|v| (v as f32).to_bits()),
    }
}

/// (size, type, bgra) for a vertex attribute.
pub fn vertex_format(format: wgt::VertexFormat) -> Option<(u32, u32, bool)> {
    use wgt::VertexFormat as V;
    let (u, s, un, sn, f) = (DkVtxAttribType_Uint, DkVtxAttribType_Sint, DkVtxAttribType_Unorm,
                             DkVtxAttribType_Snorm, DkVtxAttribType_Float);
    Some(match format {
        V::Uint8 => (DkVtxAttribSize_1x8, u, false),
        V::Uint8x2 => (DkVtxAttribSize_2x8, u, false),
        V::Uint8x4 => (DkVtxAttribSize_4x8, u, false),
        V::Sint8 => (DkVtxAttribSize_1x8, s, false),
        V::Sint8x2 => (DkVtxAttribSize_2x8, s, false),
        V::Sint8x4 => (DkVtxAttribSize_4x8, s, false),
        V::Unorm8 => (DkVtxAttribSize_1x8, un, false),
        V::Unorm8x2 => (DkVtxAttribSize_2x8, un, false),
        V::Unorm8x4 => (DkVtxAttribSize_4x8, un, false),
        V::Snorm8 => (DkVtxAttribSize_1x8, sn, false),
        V::Snorm8x2 => (DkVtxAttribSize_2x8, sn, false),
        V::Snorm8x4 => (DkVtxAttribSize_4x8, sn, false),
        V::Uint16 => (DkVtxAttribSize_1x16, u, false),
        V::Uint16x2 => (DkVtxAttribSize_2x16, u, false),
        V::Uint16x4 => (DkVtxAttribSize_4x16, u, false),
        V::Sint16 => (DkVtxAttribSize_1x16, s, false),
        V::Sint16x2 => (DkVtxAttribSize_2x16, s, false),
        V::Sint16x4 => (DkVtxAttribSize_4x16, s, false),
        V::Unorm16 => (DkVtxAttribSize_1x16, un, false),
        V::Unorm16x2 => (DkVtxAttribSize_2x16, un, false),
        V::Unorm16x4 => (DkVtxAttribSize_4x16, un, false),
        V::Snorm16 => (DkVtxAttribSize_1x16, sn, false),
        V::Snorm16x2 => (DkVtxAttribSize_2x16, sn, false),
        V::Snorm16x4 => (DkVtxAttribSize_4x16, sn, false),
        V::Float16 => (DkVtxAttribSize_1x16, f, false),
        V::Float16x2 => (DkVtxAttribSize_2x16, f, false),
        V::Float16x4 => (DkVtxAttribSize_4x16, f, false),
        V::Float32 => (DkVtxAttribSize_1x32, f, false),
        V::Float32x2 => (DkVtxAttribSize_2x32, f, false),
        V::Float32x3 => (DkVtxAttribSize_3x32, f, false),
        V::Float32x4 => (DkVtxAttribSize_4x32, f, false),
        V::Uint32 => (DkVtxAttribSize_1x32, u, false),
        V::Uint32x2 => (DkVtxAttribSize_2x32, u, false),
        V::Uint32x3 => (DkVtxAttribSize_3x32, u, false),
        V::Uint32x4 => (DkVtxAttribSize_4x32, u, false),
        V::Sint32 => (DkVtxAttribSize_1x32, s, false),
        V::Sint32x2 => (DkVtxAttribSize_2x32, s, false),
        V::Sint32x3 => (DkVtxAttribSize_3x32, s, false),
        V::Sint32x4 => (DkVtxAttribSize_4x32, s, false),
        V::Unorm10_10_10_2 => (DkVtxAttribSize_10_10_10_2, un, false),
        V::Unorm8x4Bgra => (DkVtxAttribSize_4x8, un, true),
        _ => return None,
    })
}

pub fn compare(function: wgt::CompareFunction) -> u32 {
    use wgt::CompareFunction as C;
    match function {
        C::Never => DkCompareOp_Never,
        C::Less => DkCompareOp_Less,
        C::Equal => DkCompareOp_Equal,
        C::LessEqual => DkCompareOp_Lequal,
        C::Greater => DkCompareOp_Greater,
        C::NotEqual => DkCompareOp_NotEqual,
        C::GreaterEqual => DkCompareOp_Gequal,
        C::Always => DkCompareOp_Always,
    }
}

pub fn stencil_op(op: wgt::StencilOperation) -> u32 {
    use wgt::StencilOperation as S;
    match op {
        S::Keep => DkStencilOp_Keep,
        S::Zero => DkStencilOp_Zero,
        S::Replace => DkStencilOp_Replace,
        S::Invert => DkStencilOp_Invert,
        S::IncrementClamp => DkStencilOp_Incr,
        S::DecrementClamp => DkStencilOp_Decr,
        S::IncrementWrap => DkStencilOp_IncrWrap,
        S::DecrementWrap => DkStencilOp_DecrWrap,
    }
}

/// fail, pass, depth fail, compare: PipelineDesc's stencil face order.
pub fn stencil_face(face: &wgt::StencilFaceState) -> [u32; 4] {
    [stencil_op(face.fail_op), stencil_op(face.pass_op), stencil_op(face.depth_fail_op), compare(face.compare)]
}

pub fn blend_factor(factor: wgt::BlendFactor) -> Option<u32> {
    use wgt::BlendFactor as B;
    Some(match factor {
        B::Zero => DkBlendFactor_Zero,
        B::One => DkBlendFactor_One,
        B::Src => DkBlendFactor_SrcColor,
        B::OneMinusSrc => DkBlendFactor_InvSrcColor,
        B::SrcAlpha => DkBlendFactor_SrcAlpha,
        B::OneMinusSrcAlpha => DkBlendFactor_InvSrcAlpha,
        B::Dst => DkBlendFactor_DstColor,
        B::OneMinusDst => DkBlendFactor_InvDstColor,
        B::DstAlpha => DkBlendFactor_DstAlpha,
        B::OneMinusDstAlpha => DkBlendFactor_InvDstAlpha,
        B::SrcAlphaSaturated => DkBlendFactor_SrcAlphaSaturate,
        B::Constant => DkBlendFactor_ConstColor,
        B::OneMinusConstant => DkBlendFactor_InvConstColor,
        _ => return None, // dual-source factors: the feature is not exposed
    })
}

pub fn blend_op(op: wgt::BlendOperation) -> u32 {
    use wgt::BlendOperation as B;
    match op {
        B::Add => DkBlendOp_Add,
        B::Subtract => DkBlendOp_Sub,
        B::ReverseSubtract => DkBlendOp_RevSub,
        B::Min => DkBlendOp_Min,
        B::Max => DkBlendOp_Max,
    }
}

pub fn wrap(mode: wgt::AddressMode) -> u32 {
    use wgt::AddressMode as A;
    match mode {
        A::ClampToEdge => DkWrapMode_ClampToEdge,
        A::Repeat => DkWrapMode_Repeat,
        A::MirrorRepeat => DkWrapMode_MirroredRepeat,
        A::ClampToBorder => DkWrapMode_ClampToBorder,
    }
}

pub fn filter(mode: wgt::FilterMode) -> u32 {
    match mode {
        wgt::FilterMode::Nearest => DkFilter_Nearest,
        wgt::FilterMode::Linear => DkFilter_Linear,
    }
}

pub fn mip_filter(mode: wgt::MipmapFilterMode) -> u32 {
    match mode {
        wgt::MipmapFilterMode::Nearest => DkMipFilter_Nearest,
        wgt::MipmapFilterMode::Linear => DkMipFilter_Linear,
    }
}

pub fn border(color: Option<wgt::SamplerBorderColor>) -> [f32; 4] {
    match color {
        Some(wgt::SamplerBorderColor::OpaqueBlack) => [0.0, 0.0, 0.0, 1.0],
        Some(wgt::SamplerBorderColor::OpaqueWhite) => [1.0; 4],
        _ => [0.0; 4],
    }
}

pub fn primitive(topology: wgt::PrimitiveTopology) -> u32 {
    use wgt::PrimitiveTopology as P;
    match topology {
        P::PointList => DkPrimitive_Points,
        P::LineList => DkPrimitive_Lines,
        P::LineStrip => DkPrimitive_LineStrip,
        P::TriangleList => DkPrimitive_Triangles,
        P::TriangleStrip => DkPrimitive_TriangleStrip,
    }
}

pub fn index_format(format: wgt::IndexFormat) -> u32 {
    match format {
        wgt::IndexFormat::Uint16 => DkIdxFormat_Uint16,
        wgt::IndexFormat::Uint32 => DkIdxFormat_Uint32,
    }
}

/// DkImageType for a view (or, with `samples`, a texture created with this layout).
pub fn view_type(dimension: wgt::TextureViewDimension, multisampled: bool) -> u32 {
    use wgt::TextureViewDimension as D;
    match dimension {
        D::D1 => DkImageType_1D,
        D::D2 if multisampled => DkImageType_2DMS,
        D::D2 => DkImageType_2D,
        D::D2Array if multisampled => DkImageType_2DMSArray,
        D::D2Array => DkImageType_2DArray,
        D::Cube => DkImageType_Cubemap,
        D::CubeArray => DkImageType_CubemapArray,
        D::D3 => DkImageType_3D,
    }
}
