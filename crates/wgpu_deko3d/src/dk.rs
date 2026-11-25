// Values of libnx's deko3d.h enums, and FORMAT_TRAIT_FLAGS from deko3d's maxwell/format_traits.inc,
// extracted from those files as the toolchain image ships them; values only.
#![allow(non_upper_case_globals, dead_code)]

pub const DkStageFlag_Vertex: u32 = 1;
pub const DkStageFlag_TessCtrl: u32 = 2;
pub const DkStageFlag_TessEval: u32 = 4;
pub const DkStageFlag_Geometry: u32 = 8;
pub const DkStageFlag_Fragment: u32 = 16;
pub const DkStageFlag_Compute: u32 = 32;
pub const DkStageFlag_GraphicsMask: u32 = 31;
pub const DkInvalidateFlags_Image: u32 = 1;
pub const DkInvalidateFlags_Shader: u32 = 2;
pub const DkInvalidateFlags_Descriptors: u32 = 4;
pub const DkInvalidateFlags_Zcull: u32 = 8;
pub const DkInvalidateFlags_L2Cache: u32 = 16;
pub const DkBarrier_None: u32 = 0;
pub const DkBarrier_Fragments: u32 = 2;
pub const DkBarrier_Primitives: u32 = 3;
pub const DkBarrier_Full: u32 = 4;
pub const DkImageType_None: u32 = 0;
pub const DkImageType_1D: u32 = 1;
pub const DkImageType_2D: u32 = 2;
pub const DkImageType_3D: u32 = 3;
pub const DkImageType_1DArray: u32 = 4;
pub const DkImageType_2DArray: u32 = 5;
pub const DkImageType_2DMS: u32 = 6;
pub const DkImageType_2DMSArray: u32 = 7;
pub const DkImageType_Rectangle: u32 = 8;
pub const DkImageType_Cubemap: u32 = 9;
pub const DkImageType_CubemapArray: u32 = 10;
pub const DkImageType_Buffer: u32 = 11;
pub const DkImageFlags_BlockLinear: u32 = 0;
pub const DkImageFlags_PitchLinear: u32 = 1;
pub const DkImageFlags_CustomTileSize: u32 = 2;
pub const DkImageFlags_HwCompression: u32 = 4;
pub const DkImageFlags_Z16EnableZbc: u32 = 8;
pub const DkImageFlags_UsageRender: u32 = 256;
pub const DkImageFlags_UsageLoadStore: u32 = 512;
pub const DkImageFlags_UsagePresent: u32 = 1024;
pub const DkImageFlags_Usage2DEngine: u32 = 2048;
pub const DkImageFlags_UsageVideo: u32 = 4096;
pub const DkImageFormat_None: u32 = 0;
pub const DkImageFormat_R8_Unorm: u32 = 1;
pub const DkImageFormat_R8_Snorm: u32 = 2;
pub const DkImageFormat_R8_Uint: u32 = 3;
pub const DkImageFormat_R8_Sint: u32 = 4;
pub const DkImageFormat_R16_Float: u32 = 5;
pub const DkImageFormat_R16_Unorm: u32 = 6;
pub const DkImageFormat_R16_Snorm: u32 = 7;
pub const DkImageFormat_R16_Uint: u32 = 8;
pub const DkImageFormat_R16_Sint: u32 = 9;
pub const DkImageFormat_R32_Float: u32 = 10;
pub const DkImageFormat_R32_Uint: u32 = 11;
pub const DkImageFormat_R32_Sint: u32 = 12;
pub const DkImageFormat_RG8_Unorm: u32 = 13;
pub const DkImageFormat_RG8_Snorm: u32 = 14;
pub const DkImageFormat_RG8_Uint: u32 = 15;
pub const DkImageFormat_RG8_Sint: u32 = 16;
pub const DkImageFormat_RG16_Float: u32 = 17;
pub const DkImageFormat_RG16_Unorm: u32 = 18;
pub const DkImageFormat_RG16_Snorm: u32 = 19;
pub const DkImageFormat_RG16_Uint: u32 = 20;
pub const DkImageFormat_RG16_Sint: u32 = 21;
pub const DkImageFormat_RG32_Float: u32 = 22;
pub const DkImageFormat_RG32_Uint: u32 = 23;
pub const DkImageFormat_RG32_Sint: u32 = 24;
pub const DkImageFormat_RGB32_Float: u32 = 25;
pub const DkImageFormat_RGB32_Uint: u32 = 26;
pub const DkImageFormat_RGB32_Sint: u32 = 27;
pub const DkImageFormat_RGBA8_Unorm: u32 = 28;
pub const DkImageFormat_RGBA8_Snorm: u32 = 29;
pub const DkImageFormat_RGBA8_Uint: u32 = 30;
pub const DkImageFormat_RGBA8_Sint: u32 = 31;
pub const DkImageFormat_RGBA16_Float: u32 = 32;
pub const DkImageFormat_RGBA16_Unorm: u32 = 33;
pub const DkImageFormat_RGBA16_Snorm: u32 = 34;
pub const DkImageFormat_RGBA16_Uint: u32 = 35;
pub const DkImageFormat_RGBA16_Sint: u32 = 36;
pub const DkImageFormat_RGBA32_Float: u32 = 37;
pub const DkImageFormat_RGBA32_Uint: u32 = 38;
pub const DkImageFormat_RGBA32_Sint: u32 = 39;
pub const DkImageFormat_S8: u32 = 40;
pub const DkImageFormat_Z16: u32 = 41;
pub const DkImageFormat_Z24X8: u32 = 42;
pub const DkImageFormat_ZF32: u32 = 43;
pub const DkImageFormat_Z24S8: u32 = 44;
pub const DkImageFormat_ZF32_X24S8: u32 = 45;
pub const DkImageFormat_RGBX8_Unorm_sRGB: u32 = 46;
pub const DkImageFormat_RGBA8_Unorm_sRGB: u32 = 47;
pub const DkImageFormat_RGBA4_Unorm: u32 = 48;
pub const DkImageFormat_RGB5_Unorm: u32 = 49;
pub const DkImageFormat_RGB5A1_Unorm: u32 = 50;
pub const DkImageFormat_RGB565_Unorm: u32 = 51;
pub const DkImageFormat_RGB10A2_Unorm: u32 = 52;
pub const DkImageFormat_RGB10A2_Uint: u32 = 53;
pub const DkImageFormat_RG11B10_Float: u32 = 54;
pub const DkImageFormat_E5BGR9_Float: u32 = 55;
pub const DkImageFormat_RGB_BC1: u32 = 56;
pub const DkImageFormat_RGBA_BC1: u32 = 57;
pub const DkImageFormat_RGBA_BC2: u32 = 58;
pub const DkImageFormat_RGBA_BC3: u32 = 59;
pub const DkImageFormat_RGB_BC1_sRGB: u32 = 60;
pub const DkImageFormat_RGBA_BC1_sRGB: u32 = 61;
pub const DkImageFormat_RGBA_BC2_sRGB: u32 = 62;
pub const DkImageFormat_RGBA_BC3_sRGB: u32 = 63;
pub const DkImageFormat_R_BC4_Unorm: u32 = 64;
pub const DkImageFormat_R_BC4_Snorm: u32 = 65;
pub const DkImageFormat_RG_BC5_Unorm: u32 = 66;
pub const DkImageFormat_RG_BC5_Snorm: u32 = 67;
pub const DkImageFormat_RGBA_BC7_Unorm: u32 = 68;
pub const DkImageFormat_RGBA_BC7_Unorm_sRGB: u32 = 69;
pub const DkImageFormat_RGBA_BC6H_SF16_Float: u32 = 70;
pub const DkImageFormat_RGBA_BC6H_UF16_Float: u32 = 71;
pub const DkImageFormat_RGBX8_Unorm: u32 = 72;
pub const DkImageFormat_RGBX8_Snorm: u32 = 73;
pub const DkImageFormat_RGBX8_Uint: u32 = 74;
pub const DkImageFormat_RGBX8_Sint: u32 = 75;
pub const DkImageFormat_RGBX16_Float: u32 = 76;
pub const DkImageFormat_RGBX16_Unorm: u32 = 77;
pub const DkImageFormat_RGBX16_Snorm: u32 = 78;
pub const DkImageFormat_RGBX16_Uint: u32 = 79;
pub const DkImageFormat_RGBX16_Sint: u32 = 80;
pub const DkImageFormat_RGBX32_Float: u32 = 81;
pub const DkImageFormat_RGBX32_Uint: u32 = 82;
pub const DkImageFormat_RGBX32_Sint: u32 = 83;
pub const DkImageFormat_RGBA_ASTC_4x4: u32 = 84;
pub const DkImageFormat_RGBA_ASTC_5x4: u32 = 85;
pub const DkImageFormat_RGBA_ASTC_5x5: u32 = 86;
pub const DkImageFormat_RGBA_ASTC_6x5: u32 = 87;
pub const DkImageFormat_RGBA_ASTC_6x6: u32 = 88;
pub const DkImageFormat_RGBA_ASTC_8x5: u32 = 89;
pub const DkImageFormat_RGBA_ASTC_8x6: u32 = 90;
pub const DkImageFormat_RGBA_ASTC_8x8: u32 = 91;
pub const DkImageFormat_RGBA_ASTC_10x5: u32 = 92;
pub const DkImageFormat_RGBA_ASTC_10x6: u32 = 93;
pub const DkImageFormat_RGBA_ASTC_10x8: u32 = 94;
pub const DkImageFormat_RGBA_ASTC_10x10: u32 = 95;
pub const DkImageFormat_RGBA_ASTC_12x10: u32 = 96;
pub const DkImageFormat_RGBA_ASTC_12x12: u32 = 97;
pub const DkImageFormat_RGBA_ASTC_4x4_sRGB: u32 = 98;
pub const DkImageFormat_RGBA_ASTC_5x4_sRGB: u32 = 99;
pub const DkImageFormat_RGBA_ASTC_5x5_sRGB: u32 = 100;
pub const DkImageFormat_RGBA_ASTC_6x5_sRGB: u32 = 101;
pub const DkImageFormat_RGBA_ASTC_6x6_sRGB: u32 = 102;
pub const DkImageFormat_RGBA_ASTC_8x5_sRGB: u32 = 103;
pub const DkImageFormat_RGBA_ASTC_8x6_sRGB: u32 = 104;
pub const DkImageFormat_RGBA_ASTC_8x8_sRGB: u32 = 105;
pub const DkImageFormat_RGBA_ASTC_10x5_sRGB: u32 = 106;
pub const DkImageFormat_RGBA_ASTC_10x6_sRGB: u32 = 107;
pub const DkImageFormat_RGBA_ASTC_10x8_sRGB: u32 = 108;
pub const DkImageFormat_RGBA_ASTC_10x10_sRGB: u32 = 109;
pub const DkImageFormat_RGBA_ASTC_12x10_sRGB: u32 = 110;
pub const DkImageFormat_RGBA_ASTC_12x12_sRGB: u32 = 111;
pub const DkImageFormat_BGR565_Unorm: u32 = 112;
pub const DkImageFormat_BGR5_Unorm: u32 = 113;
pub const DkImageFormat_BGR5A1_Unorm: u32 = 114;
pub const DkImageFormat_A5BGR5_Unorm: u32 = 115;
pub const DkImageFormat_BGRX8_Unorm: u32 = 116;
pub const DkImageFormat_BGRA8_Unorm: u32 = 117;
pub const DkImageFormat_BGRX8_Unorm_sRGB: u32 = 118;
pub const DkImageFormat_BGRA8_Unorm_sRGB: u32 = 119;
pub const DkImageFormat_R_ETC2_Unorm: u32 = 120;
pub const DkImageFormat_R_ETC2_Snorm: u32 = 121;
pub const DkImageFormat_RG_ETC2_Unorm: u32 = 122;
pub const DkImageFormat_RG_ETC2_Snorm: u32 = 123;
pub const DkImageFormat_RGB_ETC2: u32 = 124;
pub const DkImageFormat_RGB_PTA_ETC2: u32 = 125;
pub const DkImageFormat_RGBA_ETC2: u32 = 126;
pub const DkImageFormat_RGB_ETC2_sRGB: u32 = 127;
pub const DkImageFormat_RGB_PTA_ETC2_sRGB: u32 = 128;
pub const DkImageFormat_RGBA_ETC2_sRGB: u32 = 129;
pub const DkImageSwizzle_Zero: u32 = 0;
pub const DkImageSwizzle_One: u32 = 1;
pub const DkImageSwizzle_Red: u32 = 2;
pub const DkImageSwizzle_Green: u32 = 3;
pub const DkImageSwizzle_Blue: u32 = 4;
pub const DkImageSwizzle_Alpha: u32 = 5;
pub const DkDsSource_Depth: u32 = 0;
pub const DkDsSource_Stencil: u32 = 1;
pub const DkFilter_Nearest: u32 = 1;
pub const DkFilter_Linear: u32 = 2;
pub const DkMipFilter_None: u32 = 1;
pub const DkMipFilter_Nearest: u32 = 2;
pub const DkMipFilter_Linear: u32 = 3;
pub const DkWrapMode_Repeat: u32 = 0;
pub const DkWrapMode_MirroredRepeat: u32 = 1;
pub const DkWrapMode_ClampToEdge: u32 = 2;
pub const DkWrapMode_ClampToBorder: u32 = 3;
pub const DkWrapMode_Clamp: u32 = 4;
pub const DkWrapMode_MirrorClampToEdge: u32 = 5;
pub const DkWrapMode_MirrorClampToBorder: u32 = 6;
pub const DkWrapMode_MirrorClamp: u32 = 7;
pub const DkCompareOp_Never: u32 = 1;
pub const DkCompareOp_Less: u32 = 2;
pub const DkCompareOp_Equal: u32 = 3;
pub const DkCompareOp_Lequal: u32 = 4;
pub const DkCompareOp_Greater: u32 = 5;
pub const DkCompareOp_NotEqual: u32 = 6;
pub const DkCompareOp_Gequal: u32 = 7;
pub const DkCompareOp_Always: u32 = 8;
pub const DkPolygonMode_Point: u32 = 0;
pub const DkPolygonMode_Line: u32 = 1;
pub const DkPolygonMode_Fill: u32 = 2;
pub const DkFace_None: u32 = 0;
pub const DkFace_Front: u32 = 1;
pub const DkFace_Back: u32 = 2;
pub const DkFace_FrontAndBack: u32 = 3;
pub const DkFrontFace_CW: u32 = 0;
pub const DkFrontFace_CCW: u32 = 1;
pub const DkColorMask_R: u32 = 1;
pub const DkColorMask_G: u32 = 2;
pub const DkColorMask_B: u32 = 4;
pub const DkColorMask_A: u32 = 8;
pub const DkColorMask_RGB: u32 = 7;
pub const DkColorMask_RGBA: u32 = 15;
pub const DkBlendOp_Add: u32 = 1;
pub const DkBlendOp_Sub: u32 = 2;
pub const DkBlendOp_RevSub: u32 = 3;
pub const DkBlendOp_Min: u32 = 4;
pub const DkBlendOp_Max: u32 = 5;
pub const DkBlendFactor_Zero: u32 = 1;
pub const DkBlendFactor_One: u32 = 2;
pub const DkBlendFactor_SrcColor: u32 = 3;
pub const DkBlendFactor_InvSrcColor: u32 = 4;
pub const DkBlendFactor_SrcAlpha: u32 = 5;
pub const DkBlendFactor_InvSrcAlpha: u32 = 6;
pub const DkBlendFactor_DstAlpha: u32 = 7;
pub const DkBlendFactor_InvDstAlpha: u32 = 8;
pub const DkBlendFactor_DstColor: u32 = 9;
pub const DkBlendFactor_InvDstColor: u32 = 10;
pub const DkBlendFactor_SrcAlphaSaturate: u32 = 11;
pub const DkBlendFactor_Src1Color: u32 = 16;
pub const DkBlendFactor_InvSrc1Color: u32 = 17;
pub const DkBlendFactor_Src1Alpha: u32 = 18;
pub const DkBlendFactor_InvSrc1Alpha: u32 = 19;
pub const DkBlendFactor_ConstColor: u32 = 33;
pub const DkBlendFactor_InvConstColor: u32 = 34;
pub const DkBlendFactor_ConstAlpha: u32 = 35;
pub const DkBlendFactor_InvConstAlpha: u32 = 36;
pub const DkStencilOp_Keep: u32 = 1;
pub const DkStencilOp_Zero: u32 = 2;
pub const DkStencilOp_Replace: u32 = 3;
pub const DkStencilOp_Incr: u32 = 4;
pub const DkStencilOp_Decr: u32 = 5;
pub const DkStencilOp_Invert: u32 = 6;
pub const DkStencilOp_IncrWrap: u32 = 7;
pub const DkStencilOp_DecrWrap: u32 = 8;
pub const DkVtxAttribSize_1x32: u32 = 18;
pub const DkVtxAttribSize_2x32: u32 = 4;
pub const DkVtxAttribSize_3x32: u32 = 2;
pub const DkVtxAttribSize_4x32: u32 = 1;
pub const DkVtxAttribSize_1x16: u32 = 27;
pub const DkVtxAttribSize_2x16: u32 = 15;
pub const DkVtxAttribSize_3x16: u32 = 5;
pub const DkVtxAttribSize_4x16: u32 = 3;
pub const DkVtxAttribSize_1x8: u32 = 29;
pub const DkVtxAttribSize_2x8: u32 = 24;
pub const DkVtxAttribSize_3x8: u32 = 19;
pub const DkVtxAttribSize_4x8: u32 = 10;
pub const DkVtxAttribSize_10_10_10_2: u32 = 48;
pub const DkVtxAttribSize_11_11_10: u32 = 49;
pub const DkVtxAttribType_None: u32 = 0;
pub const DkVtxAttribType_Snorm: u32 = 1;
pub const DkVtxAttribType_Unorm: u32 = 2;
pub const DkVtxAttribType_Sint: u32 = 3;
pub const DkVtxAttribType_Uint: u32 = 4;
pub const DkVtxAttribType_Uscaled: u32 = 5;
pub const DkVtxAttribType_Sscaled: u32 = 6;
pub const DkVtxAttribType_Float: u32 = 7;
pub const DkPrimitive_Points: u32 = 0;
pub const DkPrimitive_Lines: u32 = 1;
pub const DkPrimitive_LineLoop: u32 = 2;
pub const DkPrimitive_LineStrip: u32 = 3;
pub const DkPrimitive_Triangles: u32 = 4;
pub const DkPrimitive_TriangleStrip: u32 = 5;
pub const DkPrimitive_TriangleFan: u32 = 6;
pub const DkPrimitive_Quads: u32 = 7;
pub const DkPrimitive_QuadStrip: u32 = 8;
pub const DkPrimitive_Polygon: u32 = 9;
pub const DkPrimitive_LinesAdjacency: u32 = 10;
pub const DkPrimitive_LineStripAdjacency: u32 = 11;
pub const DkPrimitive_TrianglesAdjacency: u32 = 12;
pub const DkPrimitive_TriangleStripAdjacency: u32 = 13;
pub const DkPrimitive_Patches: u32 = 14;
pub const DkIdxFormat_Uint8: u32 = 0;
pub const DkIdxFormat_Uint16: u32 = 1;
pub const DkIdxFormat_Uint32: u32 = 2;

// deko3d format trait flags (maxwell/format_traits.inc), indexed by DkImageFormat.
pub const FormatTraitFlags_CanRender: u16 = 1 << 8;
pub const FormatTraitFlags_CanLoadStore: u16 = 1 << 9;
pub const FormatTraitFlags_CanUse2DEngine: u16 = 1 << 10;
pub const FORMAT_TRAIT_FLAGS: [u16; 130] = [
    0x0000,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0701,
    0x0701,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0701,
    0x0701,
    0x0000,
    0x0001,
    0x0001,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0f04,
    0x0f00,
    0x0701,
    0x0701,
    0x0f00,
    0x0701,
    0x0701,
    0x0500,
    0x0504,
    0x0500,
    0x0508,
    0x0500,
    0x0508,
    0x0d06,
    0x0d06,
    0x0404,
    0x0c04,
    0x0c04,
    0x0c04,
    0x0f04,
    0x0701,
    0x0f00,
    0x0400,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0404,
    0x0400,
    0x0404,
    0x0400,
    0x0404,
    0x0406,
    0x0400,
    0x0400,
    0x0d04,
    0x0d00,
    0x0501,
    0x0501,
    0x0d00,
    0x0d04,
    0x0d00,
    0x0501,
    0x0501,
    0x0d00,
    0x0501,
    0x0501,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0406,
    0x0d04,
    0x0d04,
    0x0d04,
    0x0404,
    0x0d04,
    0x0f04,
    0x0d06,
    0x0d06,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0404,
    0x0406,
    0x0406,
    0x0406,
];
