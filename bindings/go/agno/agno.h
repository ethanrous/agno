#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#endif

struct AgnoImage {
  unsigned char *data;
  size_t len;
  unsigned long long width;
  unsigned long long height;
  unsigned long long page_count;
};

struct ExifData {
  unsigned char *data;
  size_t len;
  uint16_t typ;
};

void init_agno();

struct AgnoImage *load_image_from_path(char *path, size_t len);

struct AgnoImage *resize_image(struct AgnoImage *img, size_t new_width,
                               size_t new_height);

void write_agno_image_to_webp(char *path, size_t len, struct AgnoImage *img);

void free_agno_image(struct AgnoImage *img);

struct AgnoBuffer {
  unsigned char *data;
  size_t len;
};

struct AgnoBuffer write_agno_image_to_jpeg_buffer(struct AgnoImage *img,
                                                  uint8_t quality);
void free_agno_buffer(struct AgnoBuffer buf);

struct ExifData get_exif_value(struct AgnoImage *img, uint16_t img_tag);

struct GpsCoordinates {
  double lat;
  double lon;
  unsigned char valid;
};

struct GpsCoordinates get_gps_coordinates(struct AgnoImage *img);

struct AgnoImage *load_pdf_page(char *path, size_t len, size_t page_num,
                                unsigned int max_width, unsigned int max_height);

#ifdef __cplusplus
}
#endif
