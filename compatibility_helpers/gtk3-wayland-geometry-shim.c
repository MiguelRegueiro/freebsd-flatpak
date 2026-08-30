#define _GNU_SOURCE

#include <dlfcn.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

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

struct xdg_toplevel_listener_abi {
    void (*configure)(void *, void *, int32_t, int32_t, struct wl_array *);
    void (*close)(void *, void *);
    void (*configure_bounds)(void *, void *, int32_t, int32_t);
    void (*wm_capabilities)(void *, void *, struct wl_array *);
};

#define GDK_HINT_MIN_SIZE (1 << 1)
#define GDK_WINDOW_STATE_MAXIMIZED (1U << 2)
#define GDK_WINDOW_STATE_TILED (1U << 8)

#define XDG_TOPLEVEL_STATE_MAXIMIZED 1U
#define XDG_TOPLEVEL_STATE_TILED_LEFT 5U
#define XDG_TOPLEVEL_STATE_TILED_RIGHT 6U
#define XDG_TOPLEVEL_STATE_TILED_TOP 7U
#define XDG_TOPLEVEL_STATE_TILED_BOTTOM 8U

typedef void (*set_geometry_hints_fn)(void *, const struct gdk_geometry *, int);
typedef unsigned int (*get_window_state_fn)(void *);
typedef void *(*get_wayland_surface_fn)(void *);
typedef int (*add_listener_fn)(void *, void (**)(void), void *);
typedef int (*dispatcher_fn)(const void *, void *, uint32_t, const void *,
                             union wl_argument *);
typedef int (*add_dispatcher_fn)(void *, dispatcher_fn, const void *, void *);
typedef void (*destroy_proxy_fn)(void *);
typedef const char *(*get_proxy_class_fn)(void *);
typedef void (*window_lifecycle_fn)(void *);

struct saved_geometry_hints {
    void *window;
    struct gdk_geometry geometry;
    int hints;
    struct saved_geometry_hints *next;
};

struct toplevel_listener {
    void *proxy;
    void *window;
    const struct xdg_toplevel_listener_abi *original;
    void *original_data;
    struct toplevel_listener *next;
};

static set_geometry_hints_fn real_set_geometry_hints;
static get_window_state_fn get_state;
static get_wayland_surface_fn get_wayland_surface;
static add_listener_fn real_add_listener;
static add_dispatcher_fn real_add_dispatcher;
static destroy_proxy_fn real_destroy_proxy;
static get_proxy_class_fn get_proxy_class;
static window_lifecycle_fn real_window_hide;
static window_lifecycle_fn real_window_withdraw;
static window_lifecycle_fn real_window_destroy;
static struct saved_geometry_hints *saved_hints;
static struct toplevel_listener *toplevel_listeners;

#ifdef GTK3_WAYLAND_GEOMETRY_SHIM_TEST
#define TESTABLE
#else
#define TESTABLE static
#endif

TESTABLE int gtk3_xdg_toplevel_state_is_fixed(const uint32_t *states,
                                               size_t state_count)
{
    size_t index;

    for (index = 0; index < state_count; index++) {
        if (states[index] == XDG_TOPLEVEL_STATE_MAXIMIZED ||
            states[index] == XDG_TOPLEVEL_STATE_TILED_LEFT ||
            states[index] == XDG_TOPLEVEL_STATE_TILED_RIGHT ||
            states[index] == XDG_TOPLEVEL_STATE_TILED_TOP ||
            states[index] == XDG_TOPLEVEL_STATE_TILED_BOTTOM) {
            return 1;
        }
    }
    return 0;
}

TESTABLE int adjust_gtk3_geometry_hints(int hints, int is_wayland,
                                        unsigned int state)
{
    const unsigned int fixed_size =
        GDK_WINDOW_STATE_MAXIMIZED | GDK_WINDOW_STATE_TILED;

    if ((hints & GDK_HINT_MIN_SIZE) != 0 && is_wayland &&
        (state & fixed_size) != 0) {
        return hints & ~GDK_HINT_MIN_SIZE;
    }
    return hints;
}

TESTABLE int gtk3_geometry_hints_for_configure(
    int hints, const uint32_t *states, size_t state_count)
{
    return gtk3_xdg_toplevel_state_is_fixed(states, state_count)
               ? hints & ~GDK_HINT_MIN_SIZE
               : hints;
}

static struct saved_geometry_hints *find_saved_hints(void *window)
{
    struct saved_geometry_hints *saved;

    for (saved = saved_hints; saved != NULL; saved = saved->next) {
        if (saved->window == window) {
            return saved;
        }
    }
    return NULL;
}

TESTABLE int save_geometry_hints(void *window,
                                 const struct gdk_geometry *geometry,
                                 int hints)
{
    struct saved_geometry_hints *saved = find_saved_hints(window);

    if (saved == NULL) {
        saved = malloc(sizeof(*saved));
        if (saved == NULL) {
            return 0;
        }
        saved->window = window;
        saved->next = saved_hints;
        saved_hints = saved;
    }
    saved->geometry = *geometry;
    saved->hints = hints;
    return 1;
}

static void discard_saved_hints(void *window)
{
    struct saved_geometry_hints **link = &saved_hints;

    while (*link != NULL) {
        if ((*link)->window == window) {
            struct saved_geometry_hints *saved = *link;
            *link = saved->next;
            free(saved);
            return;
        }
        link = &(*link)->next;
    }
}

#ifdef GTK3_WAYLAND_GEOMETRY_SHIM_TEST
int gtk3_geometry_has_saved_hints(void *window)
{
    return find_saved_hints(window) != NULL;
}
#endif

static void apply_hints_for_configure(void *window, const uint32_t *states,
                                      size_t state_count)
{
    struct saved_geometry_hints *saved = find_saved_hints(window);
    int hints;

    if (saved == NULL || real_set_geometry_hints == NULL) {
        return;
    }
    hints =
        gtk3_geometry_hints_for_configure(saved->hints, states, state_count);
    real_set_geometry_hints(window, &saved->geometry, hints);
}

static int dispatch_toplevel_event(const void *data, void *proxy,
                                   uint32_t opcode, const void *message,
                                   union wl_argument *args)
{
    const struct toplevel_listener *listener = data;
    (void)message;

    switch (opcode) {
    case 0: {
        struct wl_array *states = args[2].a;
        const uint32_t *values = states != NULL ? states->data : NULL;
        const size_t count =
            states != NULL ? states->size / sizeof(uint32_t) : 0;

        apply_hints_for_configure(listener->window, values, count);
        listener->original->configure(listener->original_data, proxy,
                                      args[0].i, args[1].i, states);
        break;
    }
    case 1:
        listener->original->close(listener->original_data, proxy);
        break;
    case 2:
        if (listener->original->configure_bounds != NULL) {
            listener->original->configure_bounds(
                listener->original_data, proxy, args[0].i, args[1].i);
        }
        break;
    case 3:
        if (listener->original->wm_capabilities != NULL) {
            listener->original->wm_capabilities(listener->original_data,
                                                proxy, args[0].a);
        }
        break;
    default:
        return -1;
    }
    return 0;
}

TESTABLE int wrap_toplevel_listener(void *proxy,
                                    void (**implementation)(void), void *data,
                                    add_dispatcher_fn add_dispatcher)
{
    struct toplevel_listener *listener = calloc(1, sizeof(*listener));
    int result;

    if (listener == NULL) {
        return -1;
    }
    listener->proxy = proxy;
    listener->window = data;
    listener->original =
        (const struct xdg_toplevel_listener_abi *)implementation;
    listener->original_data = data;
    result = add_dispatcher(proxy, dispatch_toplevel_event, listener, data);
    if (result != 0) {
        free(listener);
        return result;
    }
    listener->next = toplevel_listeners;
    toplevel_listeners = listener;
    return 0;
}

#ifdef GTK3_WAYLAND_GEOMETRY_SHIM_TEST
size_t gtk3_geometry_tracked_listener_count(void *window)
{
    const struct toplevel_listener *listener;
    size_t count = 0;

    for (listener = toplevel_listeners; listener != NULL;
         listener = listener->next) {
        if (listener->window == window) {
            count++;
        }
    }
    return count;
}
#endif

TESTABLE void gtk3_geometry_cleanup_proxy(void *proxy)
{
    struct toplevel_listener **link = &toplevel_listeners;

    while (*link != NULL) {
        if ((*link)->proxy == proxy) {
            struct toplevel_listener *listener = *link;
            *link = listener->next;
            discard_saved_hints(listener->window);
            free(listener);
            break;
        }
        link = &(*link)->next;
    }
}

TESTABLE void gtk3_geometry_cleanup_window(void *window)
{
    struct toplevel_listener **link = &toplevel_listeners;

    while (*link != NULL) {
        if ((*link)->window == window) {
            struct toplevel_listener *listener = *link;
            *link = listener->next;
            free(listener);
        } else {
            link = &(*link)->next;
        }
    }
    discard_saved_hints(window);
}

int wl_proxy_add_listener(void *proxy, void (**implementation)(void),
                          void *data)
{
    const char *proxy_class;

    if (real_add_listener == NULL) {
        real_add_listener =
            (add_listener_fn)dlsym(RTLD_NEXT, "wl_proxy_add_listener");
        real_add_dispatcher =
            (add_dispatcher_fn)dlsym(RTLD_NEXT, "wl_proxy_add_dispatcher");
        get_proxy_class =
            (get_proxy_class_fn)dlsym(RTLD_NEXT, "wl_proxy_get_class");
    }
    if (real_add_listener == NULL) {
        return -1;
    }

    proxy_class = get_proxy_class != NULL ? get_proxy_class(proxy) : NULL;
    /* GTK passes its GdkWindow as listener data. A dispatcher forwards only
     * callbacks present in the proxy's protocol version, avoiding any
     * assumption about the size of GTK's generated listener structure. */
    if (proxy_class != NULL && strcmp(proxy_class, "xdg_toplevel") == 0 &&
        implementation != NULL && real_add_dispatcher != NULL) {
        return wrap_toplevel_listener(proxy, implementation, data,
                                      real_add_dispatcher);
    }
    return real_add_listener(proxy, implementation, data);
}

void wl_proxy_destroy(void *proxy)
{
    gtk3_geometry_cleanup_proxy(proxy);
    if (real_destroy_proxy == NULL) {
        real_destroy_proxy =
            (destroy_proxy_fn)dlsym(RTLD_NEXT, "wl_proxy_destroy");
    }
    if (real_destroy_proxy != NULL) {
        real_destroy_proxy(proxy);
    }
}

void gdk_window_hide(void *window)
{
    gtk3_geometry_cleanup_window(window);
    if (real_window_hide == NULL) {
        real_window_hide =
            (window_lifecycle_fn)dlsym(RTLD_NEXT, "gdk_window_hide");
    }
    if (real_window_hide != NULL) {
        real_window_hide(window);
    }
}

void gdk_window_withdraw(void *window)
{
    gtk3_geometry_cleanup_window(window);
    if (real_window_withdraw == NULL) {
        real_window_withdraw =
            (window_lifecycle_fn)dlsym(RTLD_NEXT, "gdk_window_withdraw");
    }
    if (real_window_withdraw != NULL) {
        real_window_withdraw(window);
    }
}

void gdk_window_destroy(void *window)
{
    gtk3_geometry_cleanup_window(window);
    if (real_window_destroy == NULL) {
        real_window_destroy =
            (window_lifecycle_fn)dlsym(RTLD_NEXT, "gdk_window_destroy");
    }
    if (real_window_destroy != NULL) {
        real_window_destroy(window);
    }
}

void gdk_window_set_geometry_hints(void *window,
                                   const struct gdk_geometry *geometry,
                                   int hints)
{
    unsigned int state = 0;
    int is_wayland = 0;
    int hints_saved = 0;

    if (real_set_geometry_hints == NULL) {
        real_set_geometry_hints = (set_geometry_hints_fn)dlsym(
            RTLD_NEXT, "gdk_window_set_geometry_hints");
    }
    if (real_set_geometry_hints == NULL) {
        return;
    }

    if (geometry != NULL) {
        hints_saved = save_geometry_hints(window, geometry, hints);
    } else {
        discard_saved_hints(window);
    }
    /* Drop the minimum immediately when GTK already knows the window is
     * tiled. This covers a later large-to-small configure even though the
     * current size is still above the minimum. */
    if (hints_saved && (hints & GDK_HINT_MIN_SIZE) != 0) {
        if (get_state == NULL) {
            get_state = (get_window_state_fn)dlsym(RTLD_DEFAULT,
                                                   "gdk_window_get_state");
            get_wayland_surface = (get_wayland_surface_fn)dlsym(
                RTLD_DEFAULT, "gdk_wayland_window_get_wl_surface");
        }
        if (get_state != NULL && get_wayland_surface != NULL) {
            state = get_state(window);
            is_wayland = get_wayland_surface(window) != NULL;
            hints = adjust_gtk3_geometry_hints(hints, is_wayland, state);
        }
    }

    real_set_geometry_hints(window, geometry, hints);
}
