// Package agno provides Go bindings for the agno image processing library.
package agno

/*
#cgo CFLAGS: -I${SRCDIR}/../../../lib
#cgo LDFLAGS: -lagno -lstdc++ -lm
#cgo darwin LDFLAGS: -framework Metal -framework QuartzCore -framework CoreGraphics
#include "agno.h"
*/
import "C"
import (
	"fmt"
	"runtime"
	"sync"
	"unsafe"
)

// Image represents an image loaded via the agno library.
// Must be closed after use. Implements io.Closer.
type Image struct {
	img   *C.struct_AgnoImage
	mu    sync.Mutex
	freed bool
}

func init() {
	C.init_agno()
}

// Open loads an image from the given file path.
func Open(path string) (*Image, error) {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	cImg := C.load_image_from_path(cPath, C.size_t(len(path)))
	if cImg == nil || cImg.len == 0 || cImg.width == 0 || cImg.height == 0 {
		return nil, fmt.Errorf("agno: failed to load image from %s", path)
	}

	img := &Image{img: cImg}
	runtime.SetFinalizer(img, func(i *Image) {
		i.Close()
	})

	return img, nil
}

// OpenPage loads a specific page from a multi-page file (e.g., PDF).
// page is 0-based. maxWidth/maxHeight of 0 uses default 2x scaling.
func OpenPage(path string, page int, maxWidth, maxHeight int) (*Image, error) {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	cImg := C.load_pdf_page(cPath, C.size_t(len(path)), C.size_t(page), C.uint(maxWidth), C.uint(maxHeight))
	if cImg == nil || cImg.len == 0 || cImg.width == 0 || cImg.height == 0 {
		return nil, fmt.Errorf("agno: failed to load page %d from %s", page, path)
	}

	img := &Image{img: cImg}
	runtime.SetFinalizer(img, func(i *Image) {
		i.Close()
	})

	return img, nil
}

// Close releases image resources. Safe to call multiple times.
func (img *Image) Close() error {
	img.mu.Lock()
	defer img.mu.Unlock()

	if img.freed {
		return nil
	}

	if img.img != nil {
		C.free_agno_image(img.img)
	}

	img.freed = true

	return nil
}

// Dimensions returns the width and height of the image.
func (img *Image) Dimensions() (width, height int) {
	return int(img.img.width), int(img.img.height)
}

// PageCount returns the number of pages in the source file.
// Returns 1 for single-page formats (JPEG, PNG, WebP, HEIC, etc.).
func (img *Image) PageCount() int {
	return int(img.img.page_count)
}

// Resize returns a new image scaled by the given factor.
// The receiver is consumed by this call and must not be used afterward.
func (img *Image) Resize(scale float64) (*Image, error) {
	img.mu.Lock()

	if img.freed {
		img.mu.Unlock()
		return nil, fmt.Errorf("agno: cannot resize a freed image")
	}

	newWidth := int(float64(img.img.width) * scale)
	newHeight := int(float64(img.img.height) * scale)

	// C.resize_image consumes the old pointer (Box::from_raw on Rust side).
	// After this call, img.img is invalid.
	newCImg := C.resize_image(img.img, C.size_t(newWidth), C.size_t(newHeight))

	// Mark receiver as freed so finalizer doesn't double-free.
	img.freed = true
	img.img = nil
	img.mu.Unlock()

	if newCImg == nil {
		return nil, fmt.Errorf("agno: resize failed")
	}

	newImg := &Image{img: newCImg}
	runtime.SetFinalizer(newImg, func(i *Image) {
		i.Close()
	})

	return newImg, nil
}

// WriteWebP writes the image as WebP to the given file path.
func (img *Image) WriteWebP(path string) error {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	img.mu.Lock()
	defer img.mu.Unlock()

	C.write_agno_image_to_webp(cPath, C.size_t(len(path)), img.img)

	return nil
}

// WriteJPEG encodes the image as JPEG with the given quality (1-100)
// and returns the encoded bytes.
func (img *Image) WriteJPEG(quality int) ([]byte, error) {
	img.mu.Lock()
	defer img.mu.Unlock()

	buf := C.write_agno_image_to_jpeg_buffer(img.img, C.uint8_t(quality))
	if buf.data == nil {
		return nil, fmt.Errorf("agno: failed to encode JPEG")
	}

	bs := C.GoBytes(unsafe.Pointer(buf.data), C.int(buf.len))
	C.free_agno_buffer(buf)

	return bs, nil
}

// GPSCoordinates extracts GPS coordinates from the image's EXIF data.
// Returns [lat, lon] as decimal degrees.
func (img *Image) GPSCoordinates() ([2]float64, error) {
	img.mu.Lock()
	defer img.mu.Unlock()

	gps := C.get_gps_coordinates(img.img)
	if gps.valid == 0 {
		return [2]float64{}, fmt.Errorf("agno: no GPS coordinates in EXIF data")
	}

	return [2]float64{float64(gps.lat), float64(gps.lon)}, nil
}

// ExifValue retrieves an EXIF tag value, converting to type T.
func ExifValue[T any](img *Image, tag ExifTag) (T, error) {
	v := img.getExifValue(int(tag))

	if v == nil {
		var zero T
		return zero, fmt.Errorf("agno: exif tag 0x%04x not found", tag)
	}

	if val, ok := v.(T); ok {
		return val, nil
	}

	var zero T
	return zero, fmt.Errorf("agno: exif tag 0x%04x: cannot convert %T to %T", tag, v, zero)
}

func (img *Image) getExifValue(exifTag int) any {
	img.mu.Lock()
	defer img.mu.Unlock()

	v := C.get_exif_value(img.img, C.uint16_t(exifTag))
	if v.len == 0 && v.data == nil {
		return nil
	}

	switch v.typ {
	case 1: // BYTE
		return int(*(*byte)(unsafe.Pointer(v.data)))
	case 2: // ASCII
		s := make([]byte, v.len)
		for i := 0; i < int(v.len); i++ {
			s[i] = *(*byte)(unsafe.Add(unsafe.Pointer(v.data), i))
		}
		return string(s)
	case 3: // SHORT
		return int(*(*uint32)(unsafe.Pointer(v.data)))
	case 4: // LONG
		return int(*(*uint32)(unsafe.Pointer(v.data)))
	case 5: // RATIONAL
		num := *(*uint32)(unsafe.Pointer(v.data))
		den := *(*uint32)(unsafe.Add(unsafe.Pointer(v.data), 4))
		if den == 0 {
			return float64(0)
		}
		return float64(num) / float64(den)
	case 7: // UNDEFINED
		s := make([]byte, v.len)
		for i := 0; i < int(v.len); i++ {
			s[i] = *(*byte)(unsafe.Add(unsafe.Pointer(v.data), i))
		}
		return string(s)
	case 9: // SLONG
		return int(*(*int32)(unsafe.Pointer(v.data)))
	case 10: // SRATIONAL
		num := *(*int32)(unsafe.Pointer(v.data))
		den := *(*int32)(unsafe.Add(unsafe.Pointer(v.data), 4))
		if den == 0 {
			return float64(0)
		}
		return float64(num) / float64(den)
	default:
		return nil
	}
}
