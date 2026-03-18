pub mod bitstream;
pub mod filter;
pub mod intra;
pub mod params;
pub mod picture;
pub mod slice;
pub mod transform;

use anyhow::{Context, Result, bail};

use bitstream::BitReader;
use params::{Pps, Sps, Vps};
use picture::Picture;
use slice::decode_slice;

/// Decode an HEVC still image from hvcC configuration data and raw HEVC bitstream.
///
/// This is the top-level entry point for HEIC image decoding.
/// `hvcc_data` contains the HEVCDecoderConfigurationRecord (from the hvcC box).
/// `bitstream` contains the raw HEVC NAL units (from the mdat box via iloc).
pub fn decode_hevc_still(hvcc_data: &[u8], bitstream: &[u8]) -> Result<Picture> {
    let (vps_nals, sps_nals, pps_nals) = parse_hvcc_nals(hvcc_data)?;

    let vps = if let Some(vps_data) = vps_nals.first() {
        let rbsp = remove_emulation_prevention(vps_data);
        let mut r = BitReader::new(&rbsp);
        Vps::parse(&mut r).context("Failed to parse VPS")?
    } else {
        bail!("No VPS found in hvcC");
    };

    let sps = if let Some(sps_data) = sps_nals.first() {
        let rbsp = remove_emulation_prevention(sps_data);
        let mut r = BitReader::new(&rbsp);
        Sps::parse(&mut r, vps.max_sub_layers_minus1)
            .context("Failed to parse SPS")?
    } else {
        bail!("No SPS found in hvcC");
    };

    let pps = if let Some(pps_data) = pps_nals.first() {
        let rbsp = remove_emulation_prevention(pps_data);
        let mut r = BitReader::new(&rbsp);
        Pps::parse(&mut r).context("Failed to parse PPS")?
    } else {
        bail!("No PPS found in hvcC");
    };

    let nal_length_size = parse_hvcc_nal_length_size(hvcc_data)?;
    let slice_nals = extract_slice_nals(bitstream, nal_length_size)?;

    if slice_nals.is_empty() {
        bail!("No slice NAL units found in bitstream");
    }

    let bit_depth = (sps.bit_depth_luma_minus8 + 8) as u8;
    let mut pic = Picture::new(
        sps.pic_width_in_luma_samples,
        sps.pic_height_in_luma_samples,
        bit_depth,
    );

    // Decode each slice (typically just one for still images)
    for slice_data in &slice_nals {
        let rbsp = remove_emulation_prevention(slice_data);
        decode_slice(&rbsp, &sps, &pps, &mut pic)
            .context("Failed to decode slice")?;
    }

    // Apply in-loop filters
    if !pps.pps_deblocking_filter_disabled_flag {
        let slice_qp = 26 + pps.init_qp_minus26;
        filter::deblock(&mut pic, &sps, &pps, slice_qp);
    }

    // SAO is applied per-CTU during slice decoding (stored in pic.sao_params)
    if sps.sample_adaptive_offset_enabled_flag {
        filter::apply_sao(&mut pic, &sps);
    }

    Ok(pic)
}

/// Parse the HEVCDecoderConfigurationRecord to extract VPS, SPS, PPS NAL unit data.
fn parse_hvcc_nals(hvcc: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    if hvcc.len() < 23 {
        bail!("hvcC too short: {} bytes", hvcc.len());
    }

    let num_arrays = hvcc[22] as usize;
    let mut pos = 23;

    let mut vps_nals = Vec::new();
    let mut sps_nals = Vec::new();
    let mut pps_nals = Vec::new();

    for _ in 0..num_arrays {
        if pos + 3 > hvcc.len() {
            break;
        }
        let nal_type = hvcc[pos] & 0x3F;
        let num_nalus = u16::from_be_bytes([hvcc[pos + 1], hvcc[pos + 2]]) as usize;
        pos += 3;

        for _ in 0..num_nalus {
            if pos + 2 > hvcc.len() {
                break;
            }
            let nal_len = u16::from_be_bytes([hvcc[pos], hvcc[pos + 1]]) as usize;
            pos += 2;

            if pos + nal_len > hvcc.len() {
                break;
            }

            let nal_data = hvcc[pos..pos + nal_len].to_vec();
            pos += nal_len;

            // Skip the 2-byte NAL unit header
            if nal_data.len() < 2 {
                continue;
            }
            let payload = nal_data[2..].to_vec();

            match nal_type {
                32 => vps_nals.push(payload), // VPS_NUT
                33 => sps_nals.push(payload), // SPS_NUT
                34 => pps_nals.push(payload), // PPS_NUT
                _ => {}
            }
        }
    }

    Ok((vps_nals, sps_nals, pps_nals))
}

/// Parse the NAL unit length size from hvcC (1, 2, or 4 bytes).
fn parse_hvcc_nal_length_size(hvcc: &[u8]) -> Result<usize> {
    if hvcc.len() < 22 {
        bail!("hvcC too short for length size");
    }
    Ok(((hvcc[21] & 0x03) + 1) as usize)
}

/// Extract slice NAL units from length-prefixed bitstream data.
fn extract_slice_nals(data: &[u8], length_size: usize) -> Result<Vec<Vec<u8>>> {
    let mut nals = Vec::new();
    let mut pos = 0;

    while pos + length_size <= data.len() {
        let nal_len = match length_size {
            1 => data[pos] as usize,
            2 => u16::from_be_bytes([data[pos], data[pos + 1]]) as usize,
            4 => u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize,
            _ => bail!("Invalid NAL length size: {}", length_size),
        };
        pos += length_size;

        if pos + nal_len > data.len() {
            break;
        }

        if nal_len >= 2 {
            let nal_type = (data[pos] >> 1) & 0x3F;
            // Slice NAL types: IDR_W_RADL=19, IDR_N_LP=20, CRA_NUT=21,
            // TRAIL_N=0, TRAIL_R=1, TSA_N=2, TSA_R=3, STSA_N=4, STSA_R=5,
            // BLA_W_LP=16, BLA_W_RADL=17, BLA_N_LP=18
            if nal_type <= 21 {
                // Skip 2-byte NAL header
                nals.push(data[pos + 2..pos + nal_len].to_vec());
            }
        }

        pos += nal_len;
    }

    Ok(nals)
}

/// Remove emulation prevention bytes (0x00 0x00 0x03 → 0x00 0x00).
fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0x00 && data[i + 1] == 0x00 && data[i + 2] == 0x03 {
            out.push(0x00);
            out.push(0x00);
            i += 3;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_emulation_prevention() {
        assert_eq!(
            remove_emulation_prevention(&[0x00, 0x00, 0x03, 0x01]),
            vec![0x00, 0x00, 0x01]
        );
        assert_eq!(
            remove_emulation_prevention(&[0x01, 0x02, 0x03]),
            vec![0x01, 0x02, 0x03]
        );
        assert_eq!(
            remove_emulation_prevention(&[0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01]),
            vec![0x00, 0x00, 0x00, 0x00, 0x01]
        );
    }

    #[test]
    fn test_parse_hvcc_nal_length_size() {
        let mut hvcc = vec![0u8; 23];
        hvcc[21] = 0xFF; // lengthSizeMinusOne = 3 → length_size = 4
        assert_eq!(parse_hvcc_nal_length_size(&hvcc).unwrap(), 4);

        hvcc[21] = 0xFC; // lengthSizeMinusOne = 0 → length_size = 1
        assert_eq!(parse_hvcc_nal_length_size(&hvcc).unwrap(), 1);
    }
}
