#include <assert.h>
#include <stdio.h>

#define GDK_HINT_MIN_SIZE (1 << 1)
#define GDK_HINT_MAX_SIZE (1 << 2)
#define GDK_WINDOW_STATE_MAXIMIZED (1U << 2)
#define GDK_WINDOW_STATE_TILED (1U << 8)

int adjust_gtk3_geometry_hints(int hints, int is_wayland, unsigned int state,
                               int width, int height, int min_width,
                               int min_height);

int main(void)
{
    const int hints = GDK_HINT_MIN_SIZE | GDK_HINT_MAX_SIZE;

    assert(adjust_gtk3_geometry_hints(
               hints, 1, GDK_WINDOW_STATE_TILED, 738, 395, 400, 500) ==
           GDK_HINT_MAX_SIZE);
    assert(adjust_gtk3_geometry_hints(
               hints, 1, GDK_WINDOW_STATE_MAXIMIZED, 738, 395, 400, 500) ==
           GDK_HINT_MAX_SIZE);
    assert(adjust_gtk3_geometry_hints(hints, 1, 0, 738, 395, 400, 500) ==
           hints);
    assert(adjust_gtk3_geometry_hints(
               hints, 0, GDK_WINDOW_STATE_TILED, 738, 395, 400, 500) ==
           hints);
    assert(adjust_gtk3_geometry_hints(
               hints, 1, GDK_WINDOW_STATE_TILED, 738, 500, 400, 500) ==
           hints);

    puts("Wayland GTK3 geometry tests passed");
    return 0;
}
