#include <assert.h>
#include <stdint.h>
#include <stdio.h>

struct gdk_geometry {
    int min_width;
    int min_height;
    int max_width;
    int max_height;
    int base_width;
    int base_height;
    int width_inc;
    int height_inc;
    double min_aspect;
    double max_aspect;
    int win_gravity;
};

struct wl_array {
    size_t size;
    size_t alloc;
    void *data;
};

union wl_argument {
    int32_t i;
    uint32_t u;
    int32_t f;
    const char *s;
    void *o;
    uint32_t n;
    struct wl_array *a;
    int32_t h;
};

typedef int (*dispatcher_fn)(const void *, void *, uint32_t, const void *,
                             union wl_argument *);
typedef int (*add_dispatcher_fn)(void *, dispatcher_fn, const void *, void *);

#define GDK_HINT_MIN_SIZE (1 << 1)
#define GDK_HINT_MAX_SIZE (1 << 2)
#define GDK_WINDOW_STATE_MAXIMIZED (1U << 2)
#define GDK_WINDOW_STATE_TILED (1U << 8)

#define XDG_TOPLEVEL_STATE_MAXIMIZED 1U
#define XDG_TOPLEVEL_STATE_ACTIVATED 4U
#define XDG_TOPLEVEL_STATE_TILED_LEFT 5U
#define XDG_TOPLEVEL_STATE_TILED_BOTTOM 8U

int adjust_gtk3_geometry_hints(int hints, int is_wayland,
                               unsigned int state);
int gtk3_xdg_toplevel_state_is_fixed(const uint32_t *states,
                                     size_t state_count);
int gtk3_geometry_hints_for_configure(int hints, const uint32_t *states,
                                      size_t state_count);
int save_geometry_hints(void *window, const struct gdk_geometry *geometry,
                        int hints);
int gtk3_geometry_has_saved_hints(void *window);
int wrap_toplevel_listener(void *proxy, void (**implementation)(void),
                           void *data, add_dispatcher_fn add_dispatcher);
size_t gtk3_geometry_tracked_listener_count(void *window);
void gtk3_geometry_cleanup_proxy(void *proxy);
void gtk3_geometry_cleanup_window(void *window);

static dispatcher_fn captured_dispatcher;
static const void *captured_dispatcher_data;
static int configure_calls;
static int close_calls;
static int bounds_calls;
static int capabilities_calls;

static int capture_dispatcher(void *proxy, dispatcher_fn dispatcher,
                              const void *dispatcher_data, void *data)
{
    assert(proxy != NULL);
    assert(data != NULL);
    captured_dispatcher = dispatcher;
    captured_dispatcher_data = dispatcher_data;
    return 0;
}

static void configured(void *data, void *proxy, int32_t width, int32_t height,
                       struct wl_array *states)
{
    assert(data != NULL);
    assert(proxy != NULL);
    assert(width == 738);
    assert(height == 395);
    assert(states != NULL);
    configure_calls++;
}

static void closed(void *data, void *proxy)
{
    assert(data != NULL);
    assert(proxy != NULL);
    close_calls++;
}

static void configured_bounds(void *data, void *proxy, int32_t width,
                              int32_t height)
{
    assert(data != NULL);
    assert(proxy != NULL);
    assert(width == 800);
    assert(height == 600);
    bounds_calls++;
}

static void capabilities(void *data, void *proxy,
                         struct wl_array *capabilities_array)
{
    assert(data != NULL);
    assert(proxy != NULL);
    assert(capabilities_array != NULL);
    capabilities_calls++;
}

static void test_listener_forwarding_and_cleanup(void)
{
    struct {
        void (*configure)(void *, void *, int32_t, int32_t,
                          struct wl_array *);
        void (*close)(void *, void *);
    } legacy_listener = {configured, closed};
    struct {
        void (*configure)(void *, void *, int32_t, int32_t,
                          struct wl_array *);
        void (*close)(void *, void *);
        void (*configure_bounds)(void *, void *, int32_t, int32_t);
        void (*wm_capabilities)(void *, void *, struct wl_array *);
    } current_listener = {configured, closed, configured_bounds, capabilities};
    struct gdk_geometry geometry = {.min_width = 400, .min_height = 500};
    uint32_t tiled[] = {XDG_TOPLEVEL_STATE_TILED_LEFT};
    struct wl_array states = {
        .size = sizeof(tiled), .alloc = sizeof(tiled), .data = tiled};
    struct wl_array capability_values = {0};
    union wl_argument args[3] = {0};
    int legacy_proxy;
    int current_proxy;
    int legacy_window;
    int current_window;

    assert(save_geometry_hints(&legacy_window, &geometry, GDK_HINT_MIN_SIZE));
    assert(wrap_toplevel_listener(
               &legacy_proxy, (void (**)(void))&legacy_listener,
               &legacy_window, capture_dispatcher) == 0);
    assert(gtk3_geometry_tracked_listener_count(&legacy_window) == 1);
    args[0].i = 738;
    args[1].i = 395;
    args[2].a = &states;
    assert(captured_dispatcher(captured_dispatcher_data, &legacy_proxy, 0,
                               NULL, args) == 0);
    assert(captured_dispatcher(captured_dispatcher_data, &legacy_proxy, 1,
                               NULL, args) == 0);
    assert(configure_calls == 1);
    assert(close_calls == 1);
    gtk3_geometry_cleanup_proxy(&legacy_proxy);
    assert(gtk3_geometry_tracked_listener_count(&legacy_window) == 0);
    assert(!gtk3_geometry_has_saved_hints(&legacy_window));

    assert(save_geometry_hints(&current_window, &geometry, GDK_HINT_MIN_SIZE));
    assert(wrap_toplevel_listener(
               &current_proxy, (void (**)(void))&current_listener,
               &current_window, capture_dispatcher) == 0);
    args[0].i = 800;
    args[1].i = 600;
    assert(captured_dispatcher(captured_dispatcher_data, &current_proxy, 2,
                               NULL, args) == 0);
    args[0].a = &capability_values;
    assert(captured_dispatcher(captured_dispatcher_data, &current_proxy, 3,
                               NULL, args) == 0);
    assert(bounds_calls == 1);
    assert(capabilities_calls == 1);
    gtk3_geometry_cleanup_window(&current_window);
    assert(gtk3_geometry_tracked_listener_count(&current_window) == 0);
    assert(!gtk3_geometry_has_saved_hints(&current_window));
}

int main(void)
{
    const int hints = GDK_HINT_MIN_SIZE | GDK_HINT_MAX_SIZE;
    const uint32_t tiled[] = {
        XDG_TOPLEVEL_STATE_ACTIVATED,
        XDG_TOPLEVEL_STATE_TILED_LEFT,
        XDG_TOPLEVEL_STATE_TILED_BOTTOM,
    };
    const uint32_t maximized[] = {XDG_TOPLEVEL_STATE_MAXIMIZED,
                                  XDG_TOPLEVEL_STATE_ACTIVATED};
    const uint32_t floating[] = {XDG_TOPLEVEL_STATE_ACTIVATED};

    /* The hint is installed while the tiled window is still larger than its
     * minimum, then a later configure shrinks it below that minimum. */
    assert(adjust_gtk3_geometry_hints(hints, 1, GDK_WINDOW_STATE_TILED) ==
           GDK_HINT_MAX_SIZE);
    assert(gtk3_geometry_hints_for_configure(
               hints, tiled, sizeof(tiled) / sizeof(tiled[0])) ==
           GDK_HINT_MAX_SIZE);

    /* Initial-map configures must make the same decision from xdg state,
     * before GDK has updated its raw window state. */
    assert(gtk3_xdg_toplevel_state_is_fixed(
        tiled, sizeof(tiled) / sizeof(tiled[0])));
    assert(gtk3_geometry_hints_for_configure(
               hints, maximized, sizeof(maximized) / sizeof(maximized[0])) ==
           GDK_HINT_MAX_SIZE);

    /* A floating configure restores the application's unmodified minimum. */
    assert(!gtk3_xdg_toplevel_state_is_fixed(
        floating, sizeof(floating) / sizeof(floating[0])));
    assert(gtk3_geometry_hints_for_configure(
               hints, floating,
               sizeof(floating) / sizeof(floating[0])) == hints);
    assert(gtk3_geometry_hints_for_configure(hints, NULL, 0) == hints);

    assert(adjust_gtk3_geometry_hints(
               hints, 1, GDK_WINDOW_STATE_MAXIMIZED) == GDK_HINT_MAX_SIZE);
    assert(adjust_gtk3_geometry_hints(hints, 1, 0) == hints);
    assert(adjust_gtk3_geometry_hints(hints, 0, GDK_WINDOW_STATE_TILED) ==
           hints);
    test_listener_forwarding_and_cleanup();

    puts("Wayland GTK3 geometry tests passed");
    return 0;
}
