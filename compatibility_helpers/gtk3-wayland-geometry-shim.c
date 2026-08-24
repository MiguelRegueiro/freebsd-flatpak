#define _GNU_SOURCE

#include <dlfcn.h>
#include <stddef.h>

struct gdk_geometry_prefix {
    int min_width;
    int min_height;
};

#define GDK_HINT_MIN_SIZE (1 << 1)
#define GDK_WINDOW_STATE_MAXIMIZED (1U << 2)
#define GDK_WINDOW_STATE_TILED (1U << 8)

#ifdef GTK3_WAYLAND_GEOMETRY_SHIM_TEST
#define TESTABLE
#else
#define TESTABLE static
#endif

/* GTK3 reapplies minimum-size hints when focus-only Wayland configures arrive.
 * A tiling compositor may intentionally assign a smaller fixed size, so keep
 * that compositor size until the window leaves its tiled/maximized state. */
TESTABLE int adjust_gtk3_geometry_hints(int hints, int is_wayland,
                                        unsigned int state, int width,
                                        int height, int min_width,
                                        int min_height)
{
    const unsigned int fixed_size =
        GDK_WINDOW_STATE_MAXIMIZED | GDK_WINDOW_STATE_TILED;
    if ((hints & GDK_HINT_MIN_SIZE) != 0 && is_wayland &&
        (state & fixed_size) != 0 &&
        (width < min_width || height < min_height)) {
        return hints & ~GDK_HINT_MIN_SIZE;
    }
    return hints;
}

void gdk_window_set_geometry_hints(
    void *window, const struct gdk_geometry_prefix *geometry, int hints)
{
    typedef void (*set_geometry_hints_fn)(
        void *, const struct gdk_geometry_prefix *, int);
    typedef unsigned int (*get_window_state_fn)(void *);
    typedef int (*get_window_dimension_fn)(void *);
    typedef void *(*get_wayland_surface_fn)(void *);
    static set_geometry_hints_fn real_set_geometry_hints;
    static get_window_state_fn get_state;
    static get_window_dimension_fn get_width;
    static get_window_dimension_fn get_height;
    static get_wayland_surface_fn get_wayland_surface;

    if (real_set_geometry_hints == NULL) {
        real_set_geometry_hints = (set_geometry_hints_fn)dlsym(
            RTLD_NEXT, "gdk_window_set_geometry_hints");
    }
    if (real_set_geometry_hints == NULL) {
        return;
    }

    if ((hints & GDK_HINT_MIN_SIZE) != 0 && geometry != NULL) {
        if (get_state == NULL) {
            get_state = (get_window_state_fn)dlsym(RTLD_DEFAULT,
                                                   "gdk_window_get_state");
            get_width = (get_window_dimension_fn)dlsym(
                RTLD_DEFAULT, "gdk_window_get_width");
            get_height = (get_window_dimension_fn)dlsym(
                RTLD_DEFAULT, "gdk_window_get_height");
            get_wayland_surface = (get_wayland_surface_fn)dlsym(
                RTLD_DEFAULT, "gdk_wayland_window_get_wl_surface");
        }
        if (get_state != NULL && get_width != NULL && get_height != NULL &&
            get_wayland_surface != NULL) {
            hints = adjust_gtk3_geometry_hints(
                hints, get_wayland_surface(window) != NULL, get_state(window),
                get_width(window), get_height(window), geometry->min_width,
                geometry->min_height);
        }
    }

    real_set_geometry_hints(window, geometry, hints);
}
