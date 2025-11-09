#![allow(non_camel_case_types, dead_code)]

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ExifSection {
    NONE = -1, // No section

    InteropIFD,
    IFD0,
    ExifIFD,
    SubIFD,
    SubIFD2,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ExifField {
    pub tag: u16,
    pub name: &'static str,
    pub section: ExifSection,
}

impl ExifField {
    pub const fn new(tag: u16, name: &'static str, section: ExifSection) -> Self {
        ExifField { tag, name, section }
    }
}

// if let Some(field) = get_exif_field(0x9003) {
//     // field.name == "DateTimeOriginal", field.section == ExifSection::Exif
// }
//
// // Or enum-first:
// if let Ok(id) = ExifTagId::try_from(0x829D) {
//     // match id { ExifTagId::FNumber => { ... } _ => {} }
// }

macro_rules! exif_tags {
    ( $( ($name:ident, $tag:expr_2021, $section:ident, $human:expr_2021) ),+ $(,)? ) => {
        #[repr(u16)]
        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        pub enum ExifTagId {
            $( $name = $tag, )+
        }

        // One static ExifField per tag (suppress naming lint for these CamelCase statics)
        $(
            #[allow(non_camel_case_types)]
            pub static $name: ExifField = ExifField::new($tag, $human, ExifSection::$section);
        )+

        #[inline]
        pub fn get_exif_field(tag: u16) -> Option<&'static ExifField> {
            match tag {
                $( $tag => Some(& $name), )+
                _ => None,
            }
        }

        impl core::convert::TryFrom<u16> for ExifTagId {
            type Error = ();
            fn try_from(value: u16) -> Result<Self, Self::Error> {
                match value {
                    $( $tag => Ok(ExifTagId::$name), )+
                    _ => Err(()),
                }
            }
        }

        pub static ALL_FIELDS: &[&'static ExifField] = &[
            $( & $name, )+
        ];
    };
}

// Declare the tags you care about here.
// Add more lines; the macro wires up everything above.
exif_tags![
    (INTEROP_INDEX, 0x0001, InteropIFD, "InteropIndex"),
    (INTEROP_VERSION, 0x0002, InteropIFD, "InteropVersion"),
    (PROCESSING_SOFTWARE, 0x000b, IFD0, "ProcessingSoftware"),
    (SUBFILE_TYPE, 0x00fe, IFD0, "SubfileType"),
    (OLD_SUBFILE_TYPE, 0x00ff, IFD0, "OldSubfileType"),
    (IMAGE_WIDTH, 0x0100, IFD0, "ImageWidth"),
    (IMAGE_HEIGHT, 0x0101, IFD0, "ImageHeight"),
    (BITS_PER_SAMPLE, 0x0102, IFD0, "BitsPerSample"),
    (COMPRESSION, 0x0103, IFD0, "Compression"),
    (
        PHOTOMETRIC_INTERPRETATION,
        0x0106,
        IFD0,
        "PhotometricInterpretation"
    ),
    (THRESHOLDING, 0x0107, IFD0, "Thresholding"),
    (CELL_WIDTH, 0x0108, IFD0, "CellWidth"),
    (CELL_LENGTH, 0x0109, IFD0, "CellLength"),
    (FILL_ORDER, 0x010a, IFD0, "FillOrder"),
    (DOCUMENT_NAME, 0x010d, IFD0, "DocumentName"),
    (IMAGE_DESCRIPTION, 0x010e, IFD0, "ImageDescription"),
    (MAKE, 0x010f, IFD0, "Make"),
    (MODEL, 0x0110, IFD0, "Model"),
    (ORIENTATION, 0x0112, IFD0, "Orientation"),
    (SAMPLES_PER_PIXEL, 0x0115, IFD0, "SamplesPerPixel"),
    (ROWS_PER_STRIP, 0x0116, IFD0, "RowsPerStrip"),
    (MIN_SAMPLE_VALUE, 0x0118, IFD0, "MinSampleValue"),
    (MAX_SAMPLE_VALUE, 0x0119, IFD0, "MaxSampleValue"),
    (XRESOLUTION, 0x011a, IFD0, "XResolution"),
    (YRESOLUTION, 0x011b, IFD0, "YResolution"),
    (PLANAR_CONFIGURATION, 0x011c, IFD0, "PlanarConfiguration"),
    (PAGE_NAME, 0x011d, IFD0, "PageName"),
    (XPOSITION, 0x011e, IFD0, "XPosition"),
    (YPOSITION, 0x011f, IFD0, "YPosition"),
    (FREE_OFFSETS, 0x0120, NONE, "FreeOffsets"),
    (FREE_BYTE_COUNTS, 0x0121, NONE, "FreeByteCounts"),
    (GRAY_RESPONSE_UNIT, 0x0122, IFD0, "GrayResponseUnit"),
    (GRAY_RESPONSE_CURVE, 0x0123, NONE, "GrayResponseCurve"),
    (T4OPTIONS, 0x0124, NONE, "T4Options"),
    (T6OPTIONS, 0x0125, NONE, "T6Options"),
    (RESOLUTION_UNIT, 0x0128, IFD0, "ResolutionUnit"),
    (PAGE_NUMBER, 0x0129, IFD0, "PageNumber"),
    (COLOR_RESPONSE_UNIT, 0x012c, NONE, "ColorResponseUnit"),
    (TRANSFER_FUNCTION, 0x012d, IFD0, "TransferFunction"),
    (SOFTWARE, 0x0131, IFD0, "Software"),
    (MODIFY_DATE, 0x0132, IFD0, "ModifyDate"),
    (ARTIST, 0x013b, IFD0, "Artist"),
    (HOST_COMPUTER, 0x013c, IFD0, "HostComputer"),
    (PREDICTOR, 0x013d, IFD0, "Predictor"),
    (WHITE_POINT, 0x013e, IFD0, "WhitePoint"),
    (
        PRIMARY_CHROMATICITIES,
        0x013f,
        IFD0,
        "PrimaryChromaticities"
    ),
    (COLOR_MAP, 0x0140, NONE, "ColorMap"),
    (HALFTONE_HINTS, 0x0141, IFD0, "HalftoneHints"),
    (TILE_WIDTH, 0x0142, IFD0, "TileWidth"),
    (TILE_LENGTH, 0x0143, IFD0, "TileLength"),
    (TILE_OFFSETS, 0x0144, NONE, "TileOffsets"),
    (TILE_BYTE_COUNTS, 0x0145, NONE, "TileByteCounts"),
    (BAD_FAX_LINES, 0x0146, NONE, "BadFaxLines"),
    (CLEAN_FAX_DATA, 0x0147, NONE, "CleanFaxData"),
    (
        CONSECUTIVE_BAD_FAX_LINES,
        0x0148,
        NONE,
        "ConsecutiveBadFaxLines"
    ),
    (INK_SET, 0x014c, IFD0, "InkSet"),
    (INK_NAMES, 0x014d, NONE, "InkNames"),
    (NUMBEROF_INKS, 0x014e, NONE, "NumberofInks"),
    (DOT_RANGE, 0x0150, NONE, "DotRange"),
    (TARGET_PRINTER, 0x0151, IFD0, "TargetPrinter"),
    (EXTRA_SAMPLES, 0x0152, NONE, "ExtraSamples"),
    (SAMPLE_FORMAT, 0x0153, SubIFD, "SampleFormat"),
    (SMIN_SAMPLE_VALUE, 0x0154, NONE, "SMinSampleValue"),
    (SMAX_SAMPLE_VALUE, 0x0155, NONE, "SMaxSampleValue"),
    (TRANSFER_RANGE, 0x0156, NONE, "TransferRange"),
    (CLIP_PATH, 0x0157, NONE, "ClipPath"),
    (XCLIP_PATH_UNITS, 0x0158, NONE, "XClipPathUnits"),
    (YCLIP_PATH_UNITS, 0x0159, NONE, "YClipPathUnits"),
    (INDEXED, 0x015a, NONE, "Indexed"),
    (JPEGTABLES, 0x015b, NONE, "JPEGTables"),
    (OPIPROXY, 0x015f, NONE, "OPIProxy"),
    (GLOBAL_PARAMETERS_IFD, 0x0190, NONE, "GlobalParametersIFD"),
    (PROFILE_TYPE, 0x0191, NONE, "ProfileType"),
    (FAX_PROFILE, 0x0192, NONE, "FaxProfile"),
    (CODING_METHODS, 0x0193, NONE, "CodingMethods"),
    (VERSION_YEAR, 0x0194, NONE, "VersionYear"),
    (MODE_NUMBER, 0x0195, NONE, "ModeNumber"),
    (DECODE, 0x01b1, NONE, "Decode"),
    (DEFAULT_IMAGE_COLOR, 0x01b2, NONE, "DefaultImageColor"),
    (T82OPTIONS, 0x01b3, NONE, "T82Options"),
    (JPEGPROC, 0x0200, NONE, "JPEGProc"),
    (JPEGRESTART_INTERVAL, 0x0203, NONE, "JPEGRestartInterval"),
    (
        JPEGLOSSLESS_PREDICTORS,
        0x0205,
        NONE,
        "JPEGLosslessPredictors"
    ),
    (JPEGPOINT_TRANSFORMS, 0x0206, NONE, "JPEGPointTransforms"),
    (JPEGQTABLES, 0x0207, NONE, "JPEGQTables"),
    (JPEGDCTABLES, 0x0208, NONE, "JPEGDCTables"),
    (JPEGACTABLES, 0x0209, NONE, "JPEGACTables"),
    (YCB_CR_COEFFICIENTS, 0x0211, IFD0, "YCbCrCoefficients"),
    (YCB_CR_SUB_SAMPLING, 0x0212, IFD0, "YCbCrSubSampling"),
    (YCB_CR_POSITIONING, 0x0213, IFD0, "YCbCrPositioning"),
    (REFERENCE_BLACK_WHITE, 0x0214, IFD0, "ReferenceBlackWhite"),
    (STRIP_ROW_COUNTS, 0x022f, NONE, "StripRowCounts"),
    (APPLICATION_NOTES, 0x02bc, IFD0, "ApplicationNotes"),
    (RENDERING_INTENT, 0x0303, NONE, "RenderingIntent"),
    (USPTOMISCELLANEOUS, 0x03e7, NONE, "USPTOMiscellaneous"),
    (
        RELATED_IMAGE_FILE_FORMAT,
        0x1000,
        InteropIFD,
        "RelatedImageFileFormat"
    ),
    (RELATED_IMAGE_WIDTH, 0x1001, InteropIFD, "RelatedImageWidth"),
    (
        RELATED_IMAGE_HEIGHT,
        0x1002,
        InteropIFD,
        "RelatedImageHeight"
    ),
    (RATING, 0x4746, IFD0, "Rating"),
    (XP_DIP_XML, 0x4747, NONE, "XP_DIP_XML"),
    (STITCH_INFO, 0x4748, NONE, "StitchInfo"),
    (RATING_PERCENT, 0x4749, IFD0, "RatingPercent"),
    (RESOLUTION_XUNIT, 0x5001, NONE, "ResolutionXUnit"),
    (RESOLUTION_YUNIT, 0x5002, NONE, "ResolutionYUnit"),
    (
        RESOLUTION_XLENGTH_UNIT,
        0x5003,
        NONE,
        "ResolutionXLengthUnit"
    ),
    (
        RESOLUTION_YLENGTH_UNIT,
        0x5004,
        NONE,
        "ResolutionYLengthUnit"
    ),
    (PRINT_FLAGS, 0x5005, NONE, "PrintFlags"),
    (PRINT_FLAGS_VERSION, 0x5006, NONE, "PrintFlagsVersion"),
    (PRINT_FLAGS_CROP, 0x5007, NONE, "PrintFlagsCrop"),
    (
        PRINT_FLAGS_BLEED_WIDTH,
        0x5008,
        NONE,
        "PrintFlagsBleedWidth"
    ),
    (
        PRINT_FLAGS_BLEED_WIDTH_SCALE,
        0x5009,
        NONE,
        "PrintFlagsBleedWidthScale"
    ),
    (HALFTONE_LPI, 0x500a, NONE, "HalftoneLPI"),
    (HALFTONE_LPIUNIT, 0x500b, NONE, "HalftoneLPIUnit"),
    (HALFTONE_DEGREE, 0x500c, NONE, "HalftoneDegree"),
    (HALFTONE_SHAPE, 0x500d, NONE, "HalftoneShape"),
    (HALFTONE_MISC, 0x500e, NONE, "HalftoneMisc"),
    (HALFTONE_SCREEN, 0x500f, NONE, "HalftoneScreen"),
    (JPEGQUALITY, 0x5010, NONE, "JPEGQuality"),
    (GRID_SIZE, 0x5011, NONE, "GridSize"),
    (THUMBNAIL_FORMAT, 0x5012, NONE, "ThumbnailFormat"),
    (THUMBNAIL_WIDTH, 0x5013, NONE, "ThumbnailWidth"),
    (THUMBNAIL_HEIGHT, 0x5014, NONE, "ThumbnailHeight"),
    (THUMBNAIL_COLOR_DEPTH, 0x5015, NONE, "ThumbnailColorDepth"),
    (THUMBNAIL_PLANES, 0x5016, NONE, "ThumbnailPlanes"),
    (THUMBNAIL_RAW_BYTES, 0x5017, NONE, "ThumbnailRawBytes"),
    (THUMBNAIL_LENGTH, 0x5018, NONE, "ThumbnailLength"),
    (
        THUMBNAIL_COMPRESSED_SIZE,
        0x5019,
        NONE,
        "ThumbnailCompressedSize"
    ),
    (
        COLOR_TRANSFER_FUNCTION,
        0x501a,
        NONE,
        "ColorTransferFunction"
    ),
    (THUMBNAIL_DATA, 0x501b, NONE, "ThumbnailData"),
    (THUMBNAIL_IMAGE_WIDTH, 0x5020, NONE, "ThumbnailImageWidth"),
    (THUMBNAIL_IMAGE_HEIGHT, 0x5021, NONE, "ThumbnailImageHeight"),
    (
        THUMBNAIL_BITS_PER_SAMPLE,
        0x5022,
        NONE,
        "ThumbnailBitsPerSample"
    ),
    (THUMBNAIL_COMPRESSION, 0x5023, NONE, "ThumbnailCompression"),
    (
        THUMBNAIL_PHOTOMETRIC_INTERP,
        0x5024,
        NONE,
        "ThumbnailPhotometricInterp"
    ),
    (THUMBNAIL_DESCRIPTION, 0x5025, NONE, "ThumbnailDescription"),
    (THUMBNAIL_EQUIP_MAKE, 0x5026, NONE, "ThumbnailEquipMake"),
    (THUMBNAIL_EQUIP_MODEL, 0x5027, NONE, "ThumbnailEquipModel"),
    (
        THUMBNAIL_STRIP_OFFSETS,
        0x5028,
        NONE,
        "ThumbnailStripOffsets"
    ),
    (THUMBNAIL_ORIENTATION, 0x5029, NONE, "ThumbnailOrientation"),
    (
        THUMBNAIL_SAMPLES_PER_PIXEL,
        0x502a,
        NONE,
        "ThumbnailSamplesPerPixel"
    ),
    (
        THUMBNAIL_ROWS_PER_STRIP,
        0x502b,
        NONE,
        "ThumbnailRowsPerStrip"
    ),
    (
        THUMBNAIL_STRIP_BYTE_COUNTS,
        0x502c,
        NONE,
        "ThumbnailStripByteCounts"
    ),
    (THUMBNAIL_RESOLUTION_X, 0x502d, NONE, "ThumbnailResolutionX"),
    (THUMBNAIL_RESOLUTION_Y, 0x502e, NONE, "ThumbnailResolutionY"),
    (
        THUMBNAIL_PLANAR_CONFIG,
        0x502f,
        NONE,
        "ThumbnailPlanarConfig"
    ),
    (
        THUMBNAIL_RESOLUTION_UNIT,
        0x5030,
        NONE,
        "ThumbnailResolutionUnit"
    ),
    (
        THUMBNAIL_TRANSFER_FUNCTION,
        0x5031,
        NONE,
        "ThumbnailTransferFunction"
    ),
    (THUMBNAIL_SOFTWARE, 0x5032, NONE, "ThumbnailSoftware"),
    (THUMBNAIL_DATE_TIME, 0x5033, NONE, "ThumbnailDateTime"),
    (THUMBNAIL_ARTIST, 0x5034, NONE, "ThumbnailArtist"),
    (THUMBNAIL_WHITE_POINT, 0x5035, NONE, "ThumbnailWhitePoint"),
    (
        THUMBNAIL_PRIMARY_CHROMATICITIES,
        0x5036,
        NONE,
        "ThumbnailPrimaryChromaticities"
    ),
    (
        THUMBNAIL_YCB_CR_COEFFICIENTS,
        0x5037,
        NONE,
        "ThumbnailYCbCrCoefficients"
    ),
    (
        THUMBNAIL_YCB_CR_SUBSAMPLING,
        0x5038,
        NONE,
        "ThumbnailYCbCrSubsampling"
    ),
    (
        THUMBNAIL_YCB_CR_POSITIONING,
        0x5039,
        NONE,
        "ThumbnailYCbCrPositioning"
    ),
    (
        THUMBNAIL_REF_BLACK_WHITE,
        0x503a,
        NONE,
        "ThumbnailRefBlackWhite"
    ),
    (THUMBNAIL_COPYRIGHT, 0x503b, NONE, "ThumbnailCopyright"),
    (LUMINANCE_TABLE, 0x5090, NONE, "LuminanceTable"),
    (CHROMINANCE_TABLE, 0x5091, NONE, "ChrominanceTable"),
    (FRAME_DELAY, 0x5100, NONE, "FrameDelay"),
    (LOOP_COUNT, 0x5101, NONE, "LoopCount"),
    (GLOBAL_PALETTE, 0x5102, NONE, "GlobalPalette"),
    (INDEX_BACKGROUND, 0x5103, NONE, "IndexBackground"),
    (INDEX_TRANSPARENT, 0x5104, NONE, "IndexTransparent"),
    (PIXEL_UNITS, 0x5110, NONE, "PixelUnits"),
    (PIXELS_PER_UNIT_X, 0x5111, NONE, "PixelsPerUnitX"),
    (PIXELS_PER_UNIT_Y, 0x5112, NONE, "PixelsPerUnitY"),
    (PALETTE_HISTOGRAM, 0x5113, NONE, "PaletteHistogram"),
    (SONY_RAW_FILE_TYPE, 0x7000, NONE, "SonyRawFileType"),
    (SONY_TONE_CURVE, 0x7010, NONE, "SonyToneCurve"),
    (
        VIGNETTING_CORRECTION,
        0x7031,
        SubIFD,
        "VignettingCorrection"
    ),
    (
        VIGNETTING_CORR_PARAMS,
        0x7032,
        SubIFD,
        "VignettingCorrParams"
    ),
    (
        CHROMATIC_ABERRATION_CORRECTION,
        0x7034,
        SubIFD,
        "ChromaticAberrationCorrection"
    ),
    (
        CHROMATIC_ABERRATION_CORR_PARAMS,
        0x7035,
        SubIFD,
        "ChromaticAberrationCorrParams"
    ),
    (
        DISTORTION_CORRECTION,
        0x7036,
        SubIFD,
        "DistortionCorrection"
    ),
    (
        DISTORTION_CORR_PARAMS,
        0x7037,
        SubIFD,
        "DistortionCorrParams"
    ),
    (SONY_RAW_IMAGE_SIZE, 0x7038, SubIFD, "SonyRawImageSize"),
    (BLACK_LEVEL, 0x7310, SubIFD, "BlackLevel"),
    (WB_RGGBLEVELS, 0x7313, SubIFD, "WB_RGGBLevels"),
    (SONY_CROP_TOP_LEFT, 0x74c7, SubIFD, "SonyCropTopLeft"),
    (SONY_CROP_SIZE, 0x74c8, SubIFD, "SonyCropSize"),
    (IMAGE_ID, 0x800d, NONE, "ImageID"),
    (WANG_TAG1, 0x80a3, NONE, "WangTag1"),
    (WANG_ANNOTATION, 0x80a4, NONE, "WangAnnotation"),
    (WANG_TAG3, 0x80a5, NONE, "WangTag3"),
    (WANG_TAG4, 0x80a6, NONE, "WangTag4"),
    (IMAGE_REFERENCE_POINTS, 0x80b9, NONE, "ImageReferencePoints"),
    (
        REGION_XFORM_TACK_POINT,
        0x80ba,
        NONE,
        "RegionXformTackPoint"
    ),
    (WARP_QUADRILATERAL, 0x80bb, NONE, "WarpQuadrilateral"),
    (AFFINE_TRANSFORM_MAT, 0x80bc, NONE, "AffineTransformMat"),
    (MATTEING, 0x80e3, NONE, "Matteing"),
    (DATA_TYPE, 0x80e4, NONE, "DataType"),
    (IMAGE_DEPTH, 0x80e5, NONE, "ImageDepth"),
    (TILE_DEPTH, 0x80e6, NONE, "TileDepth"),
    (IMAGE_FULL_WIDTH, 0x8214, NONE, "ImageFullWidth"),
    (IMAGE_FULL_HEIGHT, 0x8215, NONE, "ImageFullHeight"),
    (TEXTURE_FORMAT, 0x8216, NONE, "TextureFormat"),
    (WRAP_MODES, 0x8217, NONE, "WrapModes"),
    (FOV_COT, 0x8218, NONE, "FovCot"),
    (MATRIX_WORLD_TO_SCREEN, 0x8219, NONE, "MatrixWorldToScreen"),
    (MATRIX_WORLD_TO_CAMERA, 0x821a, NONE, "MatrixWorldToCamera"),
    (MODEL2, 0x827d, NONE, "Model2"),
    (CFAREPEAT_PATTERN_DIM, 0x828d, SubIFD, "CFARepeatPatternDim"),
    (CFAPATTERN2, 0x828e, SubIFD, "CFAPattern2"),
    (BATTERY_LEVEL, 0x828f, NONE, "BatteryLevel"),
    (KODAK_IFD, 0x8290, NONE, "KodakIFD"),
    (COPYRIGHT, 0x8298, IFD0, "Copyright"),
    (EXPOSURE_TIME, 0x829a, ExifIFD, "ExposureTime"),
    (FNUMBER, 0x829d, ExifIFD, "FNumber"),
    (MDFILE_TAG, 0x82a5, NONE, "MDFileTag"),
    (MDSCALE_PIXEL, 0x82a6, NONE, "MDScalePixel"),
    (MDCOLOR_TABLE, 0x82a7, NONE, "MDColorTable"),
    (MDLAB_NAME, 0x82a8, NONE, "MDLabName"),
    (MDSAMPLE_INFO, 0x82a9, NONE, "MDSampleInfo"),
    (MDPREP_DATE, 0x82aa, NONE, "MDPrepDate"),
    (MDPREP_TIME, 0x82ab, NONE, "MDPrepTime"),
    (MDFILE_UNITS, 0x82ac, NONE, "MDFileUnits"),
    (PIXEL_SCALE, 0x830e, IFD0, "PixelScale"),
    (ADVENT_SCALE, 0x8335, NONE, "AdventScale"),
    (ADVENT_REVISION, 0x8336, NONE, "AdventRevision"),
    (UIC1TAG, 0x835c, NONE, "UIC1Tag"),
    (UIC2TAG, 0x835d, NONE, "UIC2Tag"),
    (UIC3TAG, 0x835e, NONE, "UIC3Tag"),
    (UIC4TAG, 0x835f, NONE, "UIC4Tag"),
    (IPTC_NAA, 0x83bb, IFD0, "IPTC-NAA"),
    (INTERGRAPH_PACKET_DATA, 0x847e, NONE, "IntergraphPacketData"),
    (
        INTERGRAPH_FLAG_REGISTERS,
        0x847f,
        NONE,
        "IntergraphFlagRegisters"
    ),
    (INTERGRAPH_MATRIX, 0x8480, IFD0, "IntergraphMatrix"),
    (INGRRESERVED, 0x8481, NONE, "INGRReserved"),
    (MODEL_TIE_POINT, 0x8482, IFD0, "ModelTiePoint"),
    (SITE, 0x84e0, NONE, "Site"),
    (COLOR_SEQUENCE, 0x84e1, NONE, "ColorSequence"),
    (IT8HEADER, 0x84e2, NONE, "IT8Header"),
    (RASTER_PADDING, 0x84e3, NONE, "RasterPadding"),
    (BITS_PER_RUN_LENGTH, 0x84e4, NONE, "BitsPerRunLength"),
    (
        BITS_PER_EXTENDED_RUN_LENGTH,
        0x84e5,
        NONE,
        "BitsPerExtendedRunLength"
    ),
    (COLOR_TABLE, 0x84e6, NONE, "ColorTable"),
    (IMAGE_COLOR_INDICATOR, 0x84e7, NONE, "ImageColorIndicator"),
    (
        BACKGROUND_COLOR_INDICATOR,
        0x84e8,
        NONE,
        "BackgroundColorIndicator"
    ),
    (IMAGE_COLOR_VALUE, 0x84e9, NONE, "ImageColorValue"),
    (BACKGROUND_COLOR_VALUE, 0x84ea, NONE, "BackgroundColorValue"),
    (PIXEL_INTENSITY_RANGE, 0x84eb, NONE, "PixelIntensityRange"),
    (
        TRANSPARENCY_INDICATOR,
        0x84ec,
        NONE,
        "TransparencyIndicator"
    ),
    (
        COLOR_CHARACTERIZATION,
        0x84ed,
        NONE,
        "ColorCharacterization"
    ),
    (HCUSAGE, 0x84ee, NONE, "HCUsage"),
    (TRAP_INDICATOR, 0x84ef, NONE, "TrapIndicator"),
    (CMYKEQUIVALENT, 0x84f0, NONE, "CMYKEquivalent"),
    (SEMINFO, 0x8546, IFD0, "SEMInfo"),
    (AFCP_IPTC, 0x8568, NONE, "AFCP_IPTC"),
    (
        PIXEL_MAGIC_JBIGOPTIONS,
        0x85b8,
        NONE,
        "PixelMagicJBIGOptions"
    ),
    (JPLCARTO_IFD, 0x85d7, NONE, "JPLCartoIFD"),
    (MODEL_TRANSFORM, 0x85d8, IFD0, "ModelTransform"),
    (WB_GRGBLEVELS, 0x8602, NONE, "WB_GRGBLevels"),
    (LEAF_DATA, 0x8606, NONE, "LeafData"),
    (PHOTOSHOP_SETTINGS, 0x8649, IFD0, "PhotoshopSettings"),
    (EXIF_OFFSET, 0x8769, IFD0, "ExifOffset"),
    (ICC_PROFILE, 0x8773, IFD0, "ICC_Profile"),
    (TIFF_FXEXTENSIONS, 0x877f, NONE, "TIFF_FXExtensions"),
    (MULTI_PROFILES, 0x8780, NONE, "MultiProfiles"),
    (SHARED_DATA, 0x8781, NONE, "SharedData"),
    (T88OPTIONS, 0x8782, NONE, "T88Options"),
    (IMAGE_LAYER, 0x87ac, NONE, "ImageLayer"),
    (GEO_TIFF_DIRECTORY, 0x87af, IFD0, "GeoTiffDirectory"),
    (GEO_TIFF_DOUBLE_PARAMS, 0x87b0, IFD0, "GeoTiffDoubleParams"),
    (GEO_TIFF_ASCII_PARAMS, 0x87b1, IFD0, "GeoTiffAsciiParams"),
    (JBIGOPTIONS, 0x87be, NONE, "JBIGOptions"),
    (EXPOSURE_PROGRAM, 0x8822, ExifIFD, "ExposureProgram"),
    (SPECTRAL_SENSITIVITY, 0x8824, ExifIFD, "SpectralSensitivity"),
    (GPSINFO, 0x8825, IFD0, "GPSInfo"),
    (ISO, 0x8827, ExifIFD, "ISO"),
    (
        OPTO_ELECTRIC_CONV_FACTOR,
        0x8828,
        NONE,
        "Opto-ElectricConvFactor"
    ),
    (INTERLACE, 0x8829, NONE, "Interlace"),
    (TIME_ZONE_OFFSET, 0x882a, ExifIFD, "TimeZoneOffset"),
    (SELF_TIMER_MODE, 0x882b, ExifIFD, "SelfTimerMode"),
    (SENSITIVITY_TYPE, 0x8830, ExifIFD, "SensitivityType"),
    (
        STANDARD_OUTPUT_SENSITIVITY,
        0x8831,
        ExifIFD,
        "StandardOutputSensitivity"
    ),
    (
        RECOMMENDED_EXPOSURE_INDEX,
        0x8832,
        ExifIFD,
        "RecommendedExposureIndex"
    ),
    (ISOSPEED, 0x8833, ExifIFD, "ISOSpeed"),
    (ISOSPEED_LATITUDEYYY, 0x8834, ExifIFD, "ISOSpeedLatitudeyyy"),
    (ISOSPEED_LATITUDEZZZ, 0x8835, ExifIFD, "ISOSpeedLatitudezzz"),
    (FAX_RECV_PARAMS, 0x885c, NONE, "FaxRecvParams"),
    (FAX_SUB_ADDRESS, 0x885d, NONE, "FaxSubAddress"),
    (FAX_RECV_TIME, 0x885e, NONE, "FaxRecvTime"),
    (FEDEX_EDR, 0x8871, NONE, "FedexEDR"),
    (LEAF_SUB_IFD, 0x888a, NONE, "LeafSubIFD"),
    (EXIF_VERSION, 0x9000, ExifIFD, "ExifVersion"),
    (DATE_TIME_ORIGINAL, 0x9003, ExifIFD, "DateTimeOriginal"),
    (CREATE_DATE, 0x9004, ExifIFD, "CreateDate"),
    (
        GOOGLE_PLUS_UPLOAD_CODE,
        0x9009,
        ExifIFD,
        "GooglePlusUploadCode"
    ),
    (OFFSET_TIME, 0x9010, ExifIFD, "OffsetTime"),
    (OFFSET_TIME_ORIGINAL, 0x9011, ExifIFD, "OffsetTimeOriginal"),
    (
        OFFSET_TIME_DIGITIZED,
        0x9012,
        ExifIFD,
        "OffsetTimeDigitized"
    ),
    (
        COMPONENTS_CONFIGURATION,
        0x9101,
        ExifIFD,
        "ComponentsConfiguration"
    ),
    (
        COMPRESSED_BITS_PER_PIXEL,
        0x9102,
        ExifIFD,
        "CompressedBitsPerPixel"
    ),
    (SHUTTER_SPEED_VALUE, 0x9201, ExifIFD, "ShutterSpeedValue"),
    (APERTURE_VALUE, 0x9202, ExifIFD, "ApertureValue"),
    (BRIGHTNESS_VALUE, 0x9203, ExifIFD, "BrightnessValue"),
    (
        EXPOSURE_COMPENSATION,
        0x9204,
        ExifIFD,
        "ExposureCompensation"
    ),
    (MAX_APERTURE_VALUE, 0x9205, ExifIFD, "MaxApertureValue"),
    (SUBJECT_DISTANCE, 0x9206, ExifIFD, "SubjectDistance"),
    (METERING_MODE, 0x9207, ExifIFD, "MeteringMode"),
    (LIGHT_SOURCE, 0x9208, ExifIFD, "LightSource"),
    (FLASH, 0x9209, ExifIFD, "Flash"),
    (FOCAL_LENGTH, 0x920a, ExifIFD, "FocalLength"),
    (FLASH_ENERGY, 0x920b, NONE, "FlashEnergy"),
    (
        SPATIAL_FREQUENCY_RESPONSE,
        0x920c,
        NONE,
        "SpatialFrequencyResponse"
    ),
    (NOISE, 0x920d, NONE, "Noise"),
    (
        FOCAL_PLANE_XRESOLUTION,
        0x920e,
        NONE,
        "FocalPlaneXResolution"
    ),
    (
        FOCAL_PLANE_YRESOLUTION,
        0x920f,
        NONE,
        "FocalPlaneYResolution"
    ),
    (
        FOCAL_PLANE_RESOLUTION_UNIT,
        0x9210,
        NONE,
        "FocalPlaneResolutionUnit"
    ),
    (IMAGE_NUMBER, 0x9211, ExifIFD, "ImageNumber"),
    (
        SECURITY_CLASSIFICATION,
        0x9212,
        ExifIFD,
        "SecurityClassification"
    ),
    (IMAGE_HISTORY, 0x9213, ExifIFD, "ImageHistory"),
    (SUBJECT_AREA, 0x9214, ExifIFD, "SubjectArea"),
    (EXPOSURE_INDEX, 0x9215, NONE, "ExposureIndex"),
    (TIFF_EPSTANDARD_ID, 0x9216, NONE, "TIFF-EPStandardID"),
    (SENSING_METHOD, 0x9217, NONE, "SensingMethod"),
    (CIP3DATA_FILE, 0x923a, NONE, "CIP3DataFile"),
    (CIP3SHEET, 0x923b, NONE, "CIP3Sheet"),
    (CIP3SIDE, 0x923c, NONE, "CIP3Side"),
    (STO_NITS, 0x923f, NONE, "StoNits"),
    (USER_COMMENT, 0x9286, ExifIFD, "UserComment"),
    (SUB_SEC_TIME, 0x9290, ExifIFD, "SubSecTime"),
    (SUB_SEC_TIME_ORIGINAL, 0x9291, ExifIFD, "SubSecTimeOriginal"),
    (
        SUB_SEC_TIME_DIGITIZED,
        0x9292,
        ExifIFD,
        "SubSecTimeDigitized"
    ),
    (MSDOCUMENT_TEXT, 0x932f, NONE, "MSDocumentText"),
    (MSPROPERTY_SET_STORAGE, 0x9330, NONE, "MSPropertySetStorage"),
    (
        MSDOCUMENT_TEXT_POSITION,
        0x9331,
        NONE,
        "MSDocumentTextPosition"
    ),
    (IMAGE_SOURCE_DATA, 0x935c, IFD0, "ImageSourceData"),
    (AMBIENT_TEMPERATURE, 0x9400, ExifIFD, "AmbientTemperature"),
    (HUMIDITY, 0x9401, ExifIFD, "Humidity"),
    (PRESSURE, 0x9402, ExifIFD, "Pressure"),
    (WATER_DEPTH, 0x9403, ExifIFD, "WaterDepth"),
    (ACCELERATION, 0x9404, ExifIFD, "Acceleration"),
    (
        CAMERA_ELEVATION_ANGLE,
        0x9405,
        ExifIFD,
        "CameraElevationAngle"
    ),
    (XIAOMI_SETTINGS, 0x9999, ExifIFD, "XiaomiSettings"),
    (XIAOMI_MODEL, 0x9a00, ExifIFD, "XiaomiModel"),
    (XPTITLE, 0x9c9b, IFD0, "XPTitle"),
    (XPCOMMENT, 0x9c9c, IFD0, "XPComment"),
    (XPAUTHOR, 0x9c9d, IFD0, "XPAuthor"),
    (XPKEYWORDS, 0x9c9e, IFD0, "XPKeywords"),
    (XPSUBJECT, 0x9c9f, IFD0, "XPSubject"),
    (FLASHPIX_VERSION, 0xa000, ExifIFD, "FlashpixVersion"),
    (COLOR_SPACE, 0xa001, ExifIFD, "ColorSpace"),
    (EXIF_IMAGE_WIDTH, 0xa002, ExifIFD, "ExifImageWidth"),
    (EXIF_IMAGE_HEIGHT, 0xa003, ExifIFD, "ExifImageHeight"),
    (RELATED_SOUND_FILE, 0xa004, ExifIFD, "RelatedSoundFile"),
    (INTEROP_OFFSET, 0xa005, NONE, "InteropOffset"),
    (
        SAMSUNG_RAW_POINTERS_OFFSET,
        0xa010,
        NONE,
        "SamsungRawPointersOffset"
    ),
    (
        SAMSUNG_RAW_POINTERS_LENGTH,
        0xa011,
        NONE,
        "SamsungRawPointersLength"
    ),
    (SAMSUNG_RAW_BYTE_ORDER, 0xa101, NONE, "SamsungRawByteOrder"),
    (SAMSUNG_RAW_UNKNOWN, 0xa102, NONE, "SamsungRawUnknown?"),
    (SUBJECT_LOCATION, 0xa214, ExifIFD, "SubjectLocation"),
    (FILE_SOURCE, 0xa300, ExifIFD, "FileSource"),
    (SCENE_TYPE, 0xa301, ExifIFD, "SceneType"),
    (CFAPATTERN, 0xa302, ExifIFD, "CFAPattern"),
    (CUSTOM_RENDERED, 0xa401, ExifIFD, "CustomRendered"),
    (EXPOSURE_MODE, 0xa402, ExifIFD, "ExposureMode"),
    (WHITE_BALANCE, 0xa403, ExifIFD, "WhiteBalance"),
    (DIGITAL_ZOOM_RATIO, 0xa404, ExifIFD, "DigitalZoomRatio"),
    (
        FOCAL_LENGTH_IN35MM_FORMAT,
        0xa405,
        ExifIFD,
        "FocalLengthIn35mmFormat"
    ),
    (SCENE_CAPTURE_TYPE, 0xa406, ExifIFD, "SceneCaptureType"),
    (GAIN_CONTROL, 0xa407, ExifIFD, "GainControl"),
    (CONTRAST, 0xa408, ExifIFD, "Contrast"),
    (SATURATION, 0xa409, ExifIFD, "Saturation"),
    (SHARPNESS, 0xa40a, ExifIFD, "Sharpness"),
    (
        DEVICE_SETTING_DESCRIPTION,
        0xa40b,
        NONE,
        "DeviceSettingDescription"
    ),
    (
        SUBJECT_DISTANCE_RANGE,
        0xa40c,
        ExifIFD,
        "SubjectDistanceRange"
    ),
    (IMAGE_UNIQUE_ID, 0xa420, ExifIFD, "ImageUniqueID"),
    (OWNER_NAME, 0xa430, ExifIFD, "OwnerName"),
    (SERIAL_NUMBER, 0xa431, ExifIFD, "SerialNumber"),
    (LENS_INFO, 0xa432, ExifIFD, "LensInfo"),
    (LENS_MAKE, 0xa433, ExifIFD, "LensMake"),
    (LENS_MODEL, 0xa434, ExifIFD, "LensModel"),
    (LENS_SERIAL_NUMBER, 0xa435, ExifIFD, "LensSerialNumber"),
    (IMAGE_TITLE, 0xa436, ExifIFD, "ImageTitle"),
    (PHOTOGRAPHER, 0xa437, ExifIFD, "Photographer"),
    (IMAGE_EDITOR, 0xa438, ExifIFD, "ImageEditor"),
    (CAMERA_FIRMWARE, 0xa439, ExifIFD, "CameraFirmware"),
    (
        RAWDEVELOPING_SOFTWARE,
        0xa43a,
        ExifIFD,
        "RAWDevelopingSoftware"
    ),
    (
        IMAGE_EDITING_SOFTWARE,
        0xa43b,
        ExifIFD,
        "ImageEditingSoftware"
    ),
    (
        METADATA_EDITING_SOFTWARE,
        0xa43c,
        ExifIFD,
        "MetadataEditingSoftware"
    ),
    (COMPOSITE_IMAGE, 0xa460, ExifIFD, "CompositeImage"),
    (
        COMPOSITE_IMAGE_COUNT,
        0xa461,
        ExifIFD,
        "CompositeImageCount"
    ),
    (
        COMPOSITE_IMAGE_EXPOSURE_TIMES,
        0xa462,
        ExifIFD,
        "CompositeImageExposureTimes"
    ),
    (GDALMETADATA, 0xa480, IFD0, "GDALMetadata"),
    (GDALNO_DATA, 0xa481, IFD0, "GDALNoData"),
    (GAMMA, 0xa500, ExifIFD, "Gamma"),
    (EXPAND_SOFTWARE, 0xafc0, NONE, "ExpandSoftware"),
    (EXPAND_LENS, 0xafc1, NONE, "ExpandLens"),
    (EXPAND_FILM, 0xafc2, NONE, "ExpandFilm"),
    (EXPAND_FILTER_LENS, 0xafc3, NONE, "ExpandFilterLens"),
    (EXPAND_SCANNER, 0xafc4, NONE, "ExpandScanner"),
    (EXPAND_FLASH_LAMP, 0xafc5, NONE, "ExpandFlashLamp"),
    (HASSELBLAD_RAW_IMAGE, 0xb4c3, NONE, "HasselbladRawImage"),
    (PIXEL_FORMAT, 0xbc01, NONE, "PixelFormat"),
    (TRANSFORMATION, 0xbc02, NONE, "Transformation"),
    (UNCOMPRESSED, 0xbc03, NONE, "Uncompressed"),
    (IMAGE_TYPE, 0xbc04, NONE, "ImageType"),
    (WIDTH_RESOLUTION, 0xbc82, NONE, "WidthResolution"),
    (HEIGHT_RESOLUTION, 0xbc83, NONE, "HeightResolution"),
    (IMAGE_OFFSET, 0xbcc0, NONE, "ImageOffset"),
    (IMAGE_BYTE_COUNT, 0xbcc1, NONE, "ImageByteCount"),
    (ALPHA_OFFSET, 0xbcc2, NONE, "AlphaOffset"),
    (ALPHA_BYTE_COUNT, 0xbcc3, NONE, "AlphaByteCount"),
    (IMAGE_DATA_DISCARD, 0xbcc4, NONE, "ImageDataDiscard"),
    (ALPHA_DATA_DISCARD, 0xbcc5, NONE, "AlphaDataDiscard"),
    (OCE_SCANJOB_DESC, 0xc427, NONE, "OceScanjobDesc"),
    (
        OCE_APPLICATION_SELECTOR,
        0xc428,
        NONE,
        "OceApplicationSelector"
    ),
    (OCE_IDNUMBER, 0xc429, NONE, "OceIDNumber"),
    (OCE_IMAGE_LOGIC, 0xc42a, NONE, "OceImageLogic"),
    (ANNOTATIONS, 0xc44f, NONE, "Annotations"),
    (PRINT_IM, 0xc4a5, IFD0, "PrintIM"),
    (HASSELBLAD_XML, 0xc519, NONE, "HasselbladXML"),
    (HASSELBLAD_EXIF, 0xc51b, NONE, "HasselbladExif"),
    (ORIGINAL_FILE_NAME, 0xc573, NONE, "OriginalFileName"),
    (
        USPTOORIGINAL_CONTENT_TYPE,
        0xc580,
        NONE,
        "USPTOOriginalContentType"
    ),
    (CR2CFAPATTERN, 0xc5e0, NONE, "CR2CFAPattern"),
    (DNGVERSION, 0xc612, IFD0, "DNGVersion"),
    (DNGBACKWARD_VERSION, 0xc613, IFD0, "DNGBackwardVersion"),
    (UNIQUE_CAMERA_MODEL, 0xc614, IFD0, "UniqueCameraModel"),
    (LOCALIZED_CAMERA_MODEL, 0xc615, IFD0, "LocalizedCameraModel"),
    (CFAPLANE_COLOR, 0xc616, SubIFD, "CFAPlaneColor"),
    (CFALAYOUT, 0xc617, SubIFD, "CFALayout"),
    (LINEARIZATION_TABLE, 0xc618, SubIFD, "LinearizationTable"),
    (
        BLACK_LEVEL_REPEAT_DIM,
        0xc619,
        SubIFD,
        "BlackLevelRepeatDim"
    ),
    (BLACK_LEVEL_DELTA_H, 0xc61b, SubIFD, "BlackLevelDeltaH"),
    (BLACK_LEVEL_DELTA_V, 0xc61c, SubIFD, "BlackLevelDeltaV"),
    (WHITE_LEVEL, 0xc61d, SubIFD, "WhiteLevel"),
    (DEFAULT_SCALE, 0xc61e, SubIFD, "DefaultScale"),
    (DEFAULT_CROP_ORIGIN, 0xc61f, SubIFD, "DefaultCropOrigin"),
    (DEFAULT_CROP_SIZE, 0xc620, SubIFD, "DefaultCropSize"),
    (COLOR_MATRIX1, 0xc621, IFD0, "ColorMatrix1"),
    (COLOR_MATRIX2, 0xc622, IFD0, "ColorMatrix2"),
    (CAMERA_CALIBRATION1, 0xc623, IFD0, "CameraCalibration1"),
    (CAMERA_CALIBRATION2, 0xc624, IFD0, "CameraCalibration2"),
    (REDUCTION_MATRIX1, 0xc625, IFD0, "ReductionMatrix1"),
    (REDUCTION_MATRIX2, 0xc626, IFD0, "ReductionMatrix2"),
    (ANALOG_BALANCE, 0xc627, IFD0, "AnalogBalance"),
    (AS_SHOT_NEUTRAL, 0xc628, IFD0, "AsShotNeutral"),
    (AS_SHOT_WHITE_XY, 0xc629, IFD0, "AsShotWhiteXY"),
    (BASELINE_EXPOSURE, 0xc62a, IFD0, "BaselineExposure"),
    (BASELINE_NOISE, 0xc62b, IFD0, "BaselineNoise"),
    (BASELINE_SHARPNESS, 0xc62c, IFD0, "BaselineSharpness"),
    (BAYER_GREEN_SPLIT, 0xc62d, SubIFD, "BayerGreenSplit"),
    (LINEAR_RESPONSE_LIMIT, 0xc62e, IFD0, "LinearResponseLimit"),
    (CAMERA_SERIAL_NUMBER, 0xc62f, IFD0, "CameraSerialNumber"),
    (DNGLENS_INFO, 0xc630, IFD0, "DNGLensInfo"),
    (CHROMA_BLUR_RADIUS, 0xc631, SubIFD, "ChromaBlurRadius"),
    (ANTI_ALIAS_STRENGTH, 0xc632, SubIFD, "AntiAliasStrength"),
    (SHADOW_SCALE, 0xc633, IFD0, "ShadowScale"),
    (MAKER_NOTE_SAFETY, 0xc635, IFD0, "MakerNoteSafety"),
    (RAW_IMAGE_SEGMENTATION, 0xc640, NONE, "RawImageSegmentation"),
    (
        CALIBRATION_ILLUMINANT1,
        0xc65a,
        IFD0,
        "CalibrationIlluminant1"
    ),
    (
        CALIBRATION_ILLUMINANT2,
        0xc65b,
        IFD0,
        "CalibrationIlluminant2"
    ),
    (BEST_QUALITY_SCALE, 0xc65c, SubIFD, "BestQualityScale"),
    (RAW_DATA_UNIQUE_ID, 0xc65d, IFD0, "RawDataUniqueID"),
    (ALIAS_LAYER_METADATA, 0xc660, NONE, "AliasLayerMetadata"),
    (ORIGINAL_RAW_FILE_NAME, 0xc68b, IFD0, "OriginalRawFileName"),
    (ORIGINAL_RAW_FILE_DATA, 0xc68c, IFD0, "OriginalRawFileData"),
    (ACTIVE_AREA, 0xc68d, SubIFD, "ActiveArea"),
    (MASKED_AREAS, 0xc68e, SubIFD, "MaskedAreas"),
    (AS_SHOT_ICCPROFILE, 0xc68f, IFD0, "AsShotICCProfile"),
    (
        AS_SHOT_PRE_PROFILE_MATRIX,
        0xc690,
        IFD0,
        "AsShotPreProfileMatrix"
    ),
    (CURRENT_ICCPROFILE, 0xc691, IFD0, "CurrentICCProfile"),
    (
        CURRENT_PRE_PROFILE_MATRIX,
        0xc692,
        IFD0,
        "CurrentPreProfileMatrix"
    ),
    (
        COLORIMETRIC_REFERENCE,
        0xc6bf,
        IFD0,
        "ColorimetricReference"
    ),
    (SRAW_TYPE, 0xc6c5, IFD0, "SRawType"),
    (PANASONIC_TITLE, 0xc6d2, IFD0, "PanasonicTitle"),
    (PANASONIC_TITLE2, 0xc6d3, IFD0, "PanasonicTitle2"),
    (CAMERA_CALIBRATION_SIG, 0xc6f3, IFD0, "CameraCalibrationSig"),
    (
        PROFILE_CALIBRATION_SIG,
        0xc6f4,
        IFD0,
        "ProfileCalibrationSig"
    ),
    (PROFILE_IFD, 0xc6f5, IFD0, "ProfileIFD"),
    (AS_SHOT_PROFILE_NAME, 0xc6f6, IFD0, "AsShotProfileName"),
    (
        NOISE_REDUCTION_APPLIED,
        0xc6f7,
        SubIFD,
        "NoiseReductionApplied"
    ),
    (PROFILE_NAME, 0xc6f8, IFD0, "ProfileName"),
    (
        PROFILE_HUE_SAT_MAP_DIMS,
        0xc6f9,
        IFD0,
        "ProfileHueSatMapDims"
    ),
    (
        PROFILE_HUE_SAT_MAP_DATA1,
        0xc6fa,
        IFD0,
        "ProfileHueSatMapData1"
    ),
    (
        PROFILE_HUE_SAT_MAP_DATA2,
        0xc6fb,
        IFD0,
        "ProfileHueSatMapData2"
    ),
    (PROFILE_TONE_CURVE, 0xc6fc, IFD0, "ProfileToneCurve"),
    (PROFILE_EMBED_POLICY, 0xc6fd, IFD0, "ProfileEmbedPolicy"),
    (PROFILE_COPYRIGHT, 0xc6fe, IFD0, "ProfileCopyright"),
    (FORWARD_MATRIX1, 0xc714, IFD0, "ForwardMatrix1"),
    (FORWARD_MATRIX2, 0xc715, IFD0, "ForwardMatrix2"),
    (
        PREVIEW_APPLICATION_NAME,
        0xc716,
        IFD0,
        "PreviewApplicationName"
    ),
    (
        PREVIEW_APPLICATION_VERSION,
        0xc717,
        IFD0,
        "PreviewApplicationVersion"
    ),
    (PREVIEW_SETTINGS_NAME, 0xc718, IFD0, "PreviewSettingsName"),
    (
        PREVIEW_SETTINGS_DIGEST,
        0xc719,
        IFD0,
        "PreviewSettingsDigest"
    ),
    (PREVIEW_COLOR_SPACE, 0xc71a, IFD0, "PreviewColorSpace"),
    (PREVIEW_DATE_TIME, 0xc71b, IFD0, "PreviewDateTime"),
    (RAW_IMAGE_DIGEST, 0xc71c, IFD0, "RawImageDigest"),
    (
        ORIGINAL_RAW_FILE_DIGEST,
        0xc71d,
        IFD0,
        "OriginalRawFileDigest"
    ),
    (SUB_TILE_BLOCK_SIZE, 0xc71e, NONE, "SubTileBlockSize"),
    (ROW_INTERLEAVE_FACTOR, 0xc71f, NONE, "RowInterleaveFactor"),
    (
        PROFILE_LOOK_TABLE_DIMS,
        0xc725,
        IFD0,
        "ProfileLookTableDims"
    ),
    (
        PROFILE_LOOK_TABLE_DATA,
        0xc726,
        IFD0,
        "ProfileLookTableData"
    ),
    (OPCODE_LIST1, 0xc740, SubIFD, "OpcodeList1"),
    (OPCODE_LIST2, 0xc741, SubIFD, "OpcodeList2"),
    (OPCODE_LIST3, 0xc74e, SubIFD, "OpcodeList3"),
    (NOISE_PROFILE, 0xc761, SubIFD, "NoiseProfile"),
    (TIME_CODES, 0xc763, IFD0, "TimeCodes"),
    (FRAME_RATE, 0xc764, IFD0, "FrameRate"),
    (TSTOP, 0xc772, IFD0, "TStop"),
    (REEL_NAME, 0xc789, IFD0, "ReelName"),
    (
        ORIGINAL_DEFAULT_FINAL_SIZE,
        0xc791,
        IFD0,
        "OriginalDefaultFinalSize"
    ),
    (
        ORIGINAL_BEST_QUALITY_SIZE,
        0xc792,
        IFD0,
        "OriginalBestQualitySize"
    ),
    (
        ORIGINAL_DEFAULT_CROP_SIZE,
        0xc793,
        IFD0,
        "OriginalDefaultCropSize"
    ),
    (CAMERA_LABEL, 0xc7a1, IFD0, "CameraLabel"),
    (
        PROFILE_HUE_SAT_MAP_ENCODING,
        0xc7a3,
        IFD0,
        "ProfileHueSatMapEncoding"
    ),
    (
        PROFILE_LOOK_TABLE_ENCODING,
        0xc7a4,
        IFD0,
        "ProfileLookTableEncoding"
    ),
    (
        BASELINE_EXPOSURE_OFFSET,
        0xc7a5,
        IFD0,
        "BaselineExposureOffset"
    ),
    (DEFAULT_BLACK_RENDER, 0xc7a6, IFD0, "DefaultBlackRender"),
    (NEW_RAW_IMAGE_DIGEST, 0xc7a7, IFD0, "NewRawImageDigest"),
    (RAW_TO_PREVIEW_GAIN, 0xc7a8, IFD0, "RawToPreviewGain"),
    (CACHE_VERSION, 0xc7aa, SubIFD2, "CacheVersion"),
    (DEFAULT_USER_CROP, 0xc7b5, SubIFD, "DefaultUserCrop"),
    (NIKON_NEFINFO, 0xc7d5, NONE, "NikonNEFInfo"),
    (ZIFMETADATA, 0xc7d7, NONE, "ZIFMetadata"),
    (ZIFANNOTATIONS, 0xc7d8, NONE, "ZIFAnnotations"),
    (DEPTH_FORMAT, 0xc7e9, IFD0, "DepthFormat"),
    (DEPTH_NEAR, 0xc7ea, IFD0, "DepthNear"),
    (DEPTH_FAR, 0xc7eb, IFD0, "DepthFar"),
    (DEPTH_UNITS, 0xc7ec, IFD0, "DepthUnits"),
    (DEPTH_MEASURE_TYPE, 0xc7ed, IFD0, "DepthMeasureType"),
    (ENHANCE_PARAMS, 0xc7ee, IFD0, "EnhanceParams"),
    (
        PROFILE_GAIN_TABLE_MAP,
        0xcd2d,
        SubIFD,
        "ProfileGainTableMap"
    ),
    (SEMANTIC_NAME, 0xcd2e, SubIFD, "SemanticName"),
    (SEMANTIC_INSTANCE_ID, 0xcd30, SubIFD, "SemanticInstanceID"),
    (
        CALIBRATION_ILLUMINANT3,
        0xcd31,
        IFD0,
        "CalibrationIlluminant3"
    ),
    (CAMERA_CALIBRATION3, 0xcd32, IFD0, "CameraCalibration3"),
    (COLOR_MATRIX3, 0xcd33, IFD0, "ColorMatrix3"),
    (FORWARD_MATRIX3, 0xcd34, IFD0, "ForwardMatrix3"),
    (ILLUMINANT_DATA1, 0xcd35, IFD0, "IlluminantData1"),
    (ILLUMINANT_DATA2, 0xcd36, IFD0, "IlluminantData2"),
    (ILLUMINANT_DATA3, 0xcd37, IFD0, "IlluminantData3"),
    (MASK_SUB_AREA, 0xcd38, SubIFD, "MaskSubArea"),
    (
        PROFILE_HUE_SAT_MAP_DATA3,
        0xcd39,
        IFD0,
        "ProfileHueSatMapData3"
    ),
    (REDUCTION_MATRIX3, 0xcd3a, IFD0, "ReductionMatrix3"),
    (RGBTABLES, 0xcd3f, IFD0, "RGBTables"),
    (
        PROFILE_GAIN_TABLE_MAP2,
        0xcd40,
        IFD0,
        "ProfileGainTableMap2"
    ),
    (JUMBF, 0xcd41, NONE, "JUMBF"),
    (
        COLUMN_INTERLEAVE_FACTOR,
        0xcd43,
        SubIFD,
        "ColumnInterleaveFactor"
    ),
    (IMAGE_SEQUENCE_INFO, 0xcd44, IFD0, "ImageSequenceInfo"),
    (IMAGE_STATS, 0xcd46, IFD0, "ImageStats"),
    (PROFILE_DYNAMIC_RANGE, 0xcd47, IFD0, "ProfileDynamicRange"),
    (PROFILE_GROUP_NAME, 0xcd48, IFD0, "ProfileGroupName"),
    (JXLDISTANCE, 0xcd49, IFD0, "JXLDistance"),
    (JXLEFFORT, 0xcd4a, IFD0, "JXLEffort"),
    (JXLDECODE_SPEED, 0xcd4b, IFD0, "JXLDecodeSpeed"),
    (SEAL, 0xcea1, IFD0, "SEAL"),
    (PADDING, 0xea1c, ExifIFD, "Padding"),
    (OFFSET_SCHEMA, 0xea1d, ExifIFD, "OffsetSchema"),
    (LENS, 0xfdea, ExifIFD, "Lens"),
    (KDC_IFD, 0xfe00, NONE, "KDC_IFD"),
    (RAW_FILE, 0xfe4c, ExifIFD, "RawFile"),
    (CONVERTER, 0xfe4d, ExifIFD, "Converter"),
    (EXPOSURE, 0xfe51, ExifIFD, "Exposure"),
    (SHADOWS, 0xfe52, ExifIFD, "Shadows"),
    (BRIGHTNESS, 0xfe53, ExifIFD, "Brightness"),
    (SMOOTHNESS, 0xfe57, ExifIFD, "Smoothness"),
    (MOIRE_FILTER, 0xfe58, ExifIFD, "MoireFilter"),
];
