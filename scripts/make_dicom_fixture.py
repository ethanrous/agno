#!/usr/bin/env python3
"""Generate PHI-free DICOM test fixtures for the agno test suite.

Reads one real MR slice, strips ALL identifying information by rebuilding a
clean Part-10 file from a whitelist of pixel/rendering tags (plus one innocuous
sequence to exercise SQ-skipping), and emits an 8-bit RGB reference render that
matches the Rust decoder's window formula exactly.

Usage:
    python3 scripts/make_dicom_fixture.py ~/tmp/brain/IMG00001.dcm
"""
import math
import sys

import numpy as np
import pydicom
from pydicom.dataset import Dataset, FileDataset, FileMetaDataset
from pydicom.sequence import Sequence
from pydicom.uid import ExplicitVRLittleEndian, generate_uid

SRC = sys.argv[1] if len(sys.argv) > 1 else "/Users/ethan/tmp/brain/IMG00001.dcm"
OUT_DCM = "tests/data/mri.dcm"
OUT_RGB = "tests/data/mri-reference.rgb"

src = pydicom.dcmread(SRC)

# --- Build a clean dataset: ONLY pixel + rendering tags. No PHI. ---
ds = Dataset()
ds.SamplesPerPixel = int(src.SamplesPerPixel)
ds.PhotometricInterpretation = str(src.PhotometricInterpretation)
ds.Rows = int(src.Rows)
ds.Columns = int(src.Columns)
ds.BitsAllocated = int(src.BitsAllocated)
ds.BitsStored = int(src.BitsStored)
ds.HighBit = int(src.HighBit)
ds.PixelRepresentation = int(src.PixelRepresentation)
ds.WindowCenter = str(src.WindowCenter)
ds.WindowWidth = str(src.WindowWidth)
ds.RescaleIntercept = str(src.RescaleIntercept)
ds.RescaleSlope = str(src.RescaleSlope)
ds.PixelData = src.PixelData  # raw pixels are not identifying

# Inject one innocuous, PHI-free defined-length sequence BEFORE pixel data so
# the committed fixture exercises the parser's sequence-skipping path.
item = Dataset()
item.CodeValue = "TEST"
item.CodingSchemeDesignator = "AGNO"
item.CodeMeaning = "synthetic"
ds.ProcedureCodeSequence = Sequence([item])

# --- File meta: fresh UIDs, Explicit VR LE, no creator/source identifiers. ---
meta = FileMetaDataset()
meta.MediaStorageSOPClassUID = "1.2.840.10008.5.1.4.1.1.4"  # MR Image Storage
meta.MediaStorageSOPInstanceUID = generate_uid()
meta.TransferSyntaxUID = ExplicitVRLittleEndian

fds = FileDataset(OUT_DCM, ds, file_meta=meta, preamble=b"\x00" * 128)
fds.is_implicit_VR = False
fds.is_little_endian = True
fds.SOPClassUID = "1.2.840.10008.5.1.4.1.1.4"
fds.SOPInstanceUID = meta.MediaStorageSOPInstanceUID
fds.save_as(OUT_DCM, write_like_original=False)

# --- Verify NO PHI leaked: re-read and assert identifying tags are absent. ---
check = pydicom.dcmread(OUT_DCM)
phi_keywords = (
    "PatientName", "PatientID", "PatientBirthDate", "PatientSex", "PatientAge",
    "PatientWeight", "ReferringPhysicianName", "RequestingPhysician",
    "PerformingPhysicianName", "OperatorsName", "InstitutionName",
    "InstitutionAddress", "AccessionNumber", "StationName", "StudyDate",
    "SeriesDate", "AcquisitionDate", "ContentDate", "DeviceSerialNumber",
    "StudyID", "InstitutionalDepartmentName",
)
present = [k for k in phi_keywords if k in check]
assert not present, f"PHI leaked into fixture: {present}"
assert check.Rows == ds.Rows and check.Columns == ds.Columns
print(f"wrote {OUT_DCM}: {check.Rows}x{check.Columns}, no PHI keywords present")

# --- Reference render: modality LUT -> linear window -> 8-bit, MATCHING Rust. ---
# pydicom's pixel_array uses the BitsAllocated-wide dtype and does NOT clamp to
# BitsStored. The Rust decoder masks (unsigned) or sign-extends (signed) from
# BitsStored, so replicate that here or the reference diverges on real data whose
# high-order bits above BitsStored are set, or on signed sub-word pixels.
bits_stored = int(check.BitsStored)
bits_allocated = int(check.BitsAllocated)
signed = int(check.PixelRepresentation) == 1
raw = check.pixel_array.astype(np.int64)
if signed:
    # Sign-extend from BitsStored: interpret the low BitsStored bits as two's
    # complement (e.g. 0x0FFF with BitsStored=12 -> -1).
    sign_bit = 1 << (bits_stored - 1)
    mask = (1 << bits_stored) - 1
    raw &= mask
    raw = (raw ^ sign_bit) - sign_bit
else:
    raw &= (1 << bits_stored) - 1
arr = raw.astype(np.float64)

slope = float(check.RescaleSlope)
intercept = float(check.RescaleIntercept)
mod = arr * slope + intercept

# Window: prefer the dataset's WindowCenter/Width; otherwise auto-window the
# modality min/max, matching the Rust decoder's fallback.
def first(v):
    return float(str(v).split("\\")[0]) if "\\" in str(v) else float(v)
if "WindowCenter" in check and "WindowWidth" in check:
    c = first(check.WindowCenter)
    w = max(first(check.WindowWidth), 1.0)
else:
    lo_v, hi_v = float(mod.min()), float(mod.max())
    w = max(hi_v - lo_v, 1.0)
    c = lo_v + w / 2.0

lo = c - 0.5 - (w - 1) / 2.0
hi = c - 0.5 + (w - 1) / 2.0
y = np.where(
    mod <= lo, 0.0,
    np.where(mod > hi, 1.0, (mod - (c - 0.5)) / (w - 1) + 0.5),
)
gray = np.clip(np.floor(y * 255.0 + 0.5), 0, 255).astype(np.uint8)
if str(check.PhotometricInterpretation) == "MONOCHROME1":
    gray = 255 - gray
rgb = np.repeat(gray[:, :, None], 3, axis=2)  # replicate to RGB
rgb.tofile(OUT_RGB)
print(f"wrote {OUT_RGB}: {rgb.size} bytes ({rgb.shape})")
