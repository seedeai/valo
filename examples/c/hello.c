/* valo from C: record a display list, render it headless, write a PPM.
 *
 * Build and run with ./examples/c/run.sh, or by hand:
 *   cargo build -p valo-capi --release
 *   cc -I crates/valo-capi/include examples/c/hello.c \
 *      -L target/release -lvalo_capi -o target/hello_c
 *   ./target/hello_c
 */
#include <stdio.h>
#include <stdlib.h>

#include "valo.h"

#define WIDTH 480
#define HEIGHT 320

/* Paint defaults every field, so a partially-filled struct never inherits
 * stale stroke or blur settings. */
static ValoPaint fill(float red, float green, float blue, float alpha) {
  ValoPaint paint = {0};
  paint.color = (ValoColor){red, green, blue, alpha};
  paint.blend_mode = 3; /* srcOver */
  paint.style = 0;      /* fill */
  paint.stroke_miter_limit = 4.0f;
  paint.mask_blur_style = 0;
  return paint;
}

static int write_ppm(const char *path, const uint8_t *rgba) {
  FILE *file = fopen(path, "wb");
  if (!file) {
    return 0;
  }
  fprintf(file, "P6\n%d %d\n255\n", WIDTH, HEIGHT);
  for (int i = 0; i < WIDTH * HEIGHT; i++) {
    fwrite(&rgba[i * 4], 1, 3, file); /* drop alpha; PPM is RGB */
  }
  fclose(file);
  return 1;
}

int main(void) {
  ValoContext *context = valo_context_new();
  if (!context) {
    fprintf(stderr, "no GPU adapter available\n");
    return 1;
  }

  ValoDisplayListBuilder *builder = valo_builder_new();

  ValoCornerRadii radii = {24, 24, 24, 24, 24, 24, 24, 24};
  valo_builder_draw_rounded_rect(builder, (ValoRect){40, 40, 400, 240}, radii,
                                 fill(0.13f, 0.15f, 0.20f, 1.0f));
  valo_builder_draw_rect(builder, (ValoRect){80, 80, 160, 60},
                         fill(0.96f, 0.35f, 0.25f, 1.0f));
  valo_builder_draw_circle(builder, 330.0f, 180.0f, 70.0f,
                           fill(0.30f, 0.75f, 0.95f, 0.85f));

  /* The builder is consumed by build(); the list it returns is retained and
   * can be rendered many times. */
  ValoDisplayList *list = valo_builder_build(builder);

  uint8_t *pixels = malloc((size_t)WIDTH * HEIGHT * 4);
  ValoColor white = {1.0f, 1.0f, 1.0f, 1.0f};
  bool ok = valo_context_render_to_pixels(context, list, white, WIDTH, HEIGHT,
                                          pixels);

  int status = 1;
  if (!ok) {
    fprintf(stderr, "render failed\n");
  } else if (!write_ppm("target/hello_c.ppm", pixels)) {
    fprintf(stderr, "could not write target/hello_c.ppm\n");
  } else {
    printf("wrote target/hello_c.ppm (%dx%d)\n", WIDTH, HEIGHT);
    status = 0;
  }

  free(pixels);
  valo_display_list_dispose(list);
  valo_context_dispose(context);
  return status;
}
