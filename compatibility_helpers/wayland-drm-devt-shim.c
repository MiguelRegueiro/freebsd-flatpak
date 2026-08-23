#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DRM_DEVT_MAP_ENV "FREEBSD_FLATPAK_DRM_DEV_T_MAP"
#define MAX_DEVT_MAPPINGS 16

struct wl_proxy;

struct wl_array {
    size_t size;
    size_t alloc;
    void *data;
};

typedef int (*wl_proxy_add_listener_fn)(struct wl_proxy *proxy,
                                        void (**implementation)(void),
                                        void *data);
typedef const char *(*wl_proxy_get_class_fn)(struct wl_proxy *proxy);

typedef void (*feedback_done_fn)(void *data, void *feedback);
typedef void (*feedback_format_table_fn)(void *data, void *feedback, int32_t fd,
                                         uint32_t size);
typedef void (*feedback_main_device_fn)(void *data, void *feedback,
                                        struct wl_array *device);
typedef void (*feedback_tranche_done_fn)(void *data, void *feedback);
typedef void (*feedback_tranche_target_device_fn)(void *data, void *feedback,
                                                  struct wl_array *device);
typedef void (*feedback_tranche_formats_fn)(void *data, void *feedback,
                                            struct wl_array *indices);
typedef void (*feedback_tranche_flags_fn)(void *data, void *feedback,
                                          uint32_t flags);

struct devt_mapping {
    uint64_t host;
    uint64_t target;
};

struct feedback_listener_state {
    void *data;
    feedback_done_fn done;
    feedback_format_table_fn format_table;
    feedback_main_device_fn main_device;
    feedback_tranche_done_fn tranche_done;
    feedback_tranche_target_device_fn tranche_target_device;
    feedback_tranche_formats_fn tranche_formats;
    feedback_tranche_flags_fn tranche_flags;
};

static struct devt_mapping devt_mappings[MAX_DEVT_MAPPINGS];
static size_t devt_mapping_count;
static int devt_mappings_loaded;

static wl_proxy_add_listener_fn real_wl_proxy_add_listener;
static wl_proxy_get_class_fn real_wl_proxy_get_class;

static int parse_u64(const char *text, uint64_t *value)
{
    char *end = NULL;

    if (text == NULL || *text == '\0') {
        return 0;
    }

    errno = 0;
    unsigned long long parsed = strtoull(text, &end, 0);
    if (errno != 0 || end == text || *end != '\0') {
        return 0;
    }

    *value = (uint64_t)parsed;
    return 1;
}

static void load_devt_mappings(void)
{
    if (devt_mappings_loaded) {
        return;
    }
    devt_mappings_loaded = 1;

    const char *env = getenv(DRM_DEVT_MAP_ENV);
    if (env == NULL || *env == '\0') {
        return;
    }

    char *copy = strdup(env);
    if (copy == NULL) {
        return;
    }

    char *save = NULL;
    for (char *entry = strtok_r(copy, ",", &save); entry != NULL;
         entry = strtok_r(NULL, ",", &save)) {
        if (devt_mapping_count >= MAX_DEVT_MAPPINGS) {
            break;
        }

        char *separator = strchr(entry, '=');
        if (separator == NULL) {
            continue;
        }
        *separator = '\0';

        uint64_t host = 0;
        uint64_t target = 0;
        if (!parse_u64(entry, &host) || !parse_u64(separator + 1, &target)) {
            continue;
        }

        devt_mappings[devt_mapping_count].host = host;
        devt_mappings[devt_mapping_count].target = target;
        devt_mapping_count++;
    }

    free(copy);
}

static int map_devt(uint64_t host, uint64_t *target)
{
    load_devt_mappings();

    for (size_t i = 0; i < devt_mapping_count; i++) {
        if (devt_mappings[i].host == host) {
            *target = devt_mappings[i].target;
            return 1;
        }
    }

    return 0;
}

static struct wl_array *rewrite_device_array(struct wl_array *device,
                                             struct wl_array *rewritten,
                                             uint64_t *rewritten_value)
{
    if (device == NULL || device->data == NULL) {
        return device;
    }

    uint64_t host = 0;
    if (device->size >= sizeof(host)) {
        memcpy(&host, device->data, sizeof(host));
    } else if (device->size == sizeof(uint32_t)) {
        uint32_t host32 = 0;
        memcpy(&host32, device->data, sizeof(host32));
        host = host32;
    } else {
        return device;
    }

    uint64_t target = 0;
    if (!map_devt(host, &target) || target == host) {
        return device;
    }

    *rewritten_value = target;
    *rewritten = *device;
    rewritten->size = sizeof(*rewritten_value);
    rewritten->data = rewritten_value;
    return rewritten;
}

static void feedback_done(void *data, void *feedback)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->done != NULL) {
        state->done(state->data, feedback);
    }
}

static void feedback_format_table(void *data, void *feedback, int32_t fd,
                                  uint32_t size)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->format_table != NULL) {
        state->format_table(state->data, feedback, fd, size);
    }
}

static void feedback_main_device(void *data, void *feedback,
                                 struct wl_array *device)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->main_device != NULL) {
        struct wl_array rewritten;
        uint64_t rewritten_value = 0;
        state->main_device(state->data, feedback,
                           rewrite_device_array(device, &rewritten,
                                                &rewritten_value));
    }
}

static void feedback_tranche_done(void *data, void *feedback)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->tranche_done != NULL) {
        state->tranche_done(state->data, feedback);
    }
}

static void feedback_tranche_target_device(void *data, void *feedback,
                                           struct wl_array *device)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->tranche_target_device != NULL) {
        struct wl_array rewritten;
        uint64_t rewritten_value = 0;
        state->tranche_target_device(
            state->data, feedback,
            rewrite_device_array(device, &rewritten, &rewritten_value));
    }
}

static void feedback_tranche_formats(void *data, void *feedback,
                                     struct wl_array *indices)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->tranche_formats != NULL) {
        state->tranche_formats(state->data, feedback, indices);
    }
}

static void feedback_tranche_flags(void *data, void *feedback, uint32_t flags)
{
    struct feedback_listener_state *state = data;
    if (state != NULL && state->tranche_flags != NULL) {
        state->tranche_flags(state->data, feedback, flags);
    }
}

static void (*feedback_listener[])(void) = {
    (void (*)(void))feedback_done,
    (void (*)(void))feedback_format_table,
    (void (*)(void))feedback_main_device,
    (void (*)(void))feedback_tranche_done,
    (void (*)(void))feedback_tranche_target_device,
    (void (*)(void))feedback_tranche_formats,
    (void (*)(void))feedback_tranche_flags,
};

static int resolve_wayland_symbols(void)
{
    if (real_wl_proxy_add_listener == NULL) {
        real_wl_proxy_add_listener =
            (wl_proxy_add_listener_fn)dlsym(RTLD_NEXT, "wl_proxy_add_listener");
    }
    if (real_wl_proxy_get_class == NULL) {
        real_wl_proxy_get_class =
            (wl_proxy_get_class_fn)dlsym(RTLD_NEXT, "wl_proxy_get_class");
    }

    return real_wl_proxy_add_listener != NULL;
}

int wl_proxy_add_listener(struct wl_proxy *proxy, void (**implementation)(void),
                          void *data)
{
    if (!resolve_wayland_symbols()) {
        return -1;
    }

    if (implementation == NULL || real_wl_proxy_get_class == NULL) {
        return real_wl_proxy_add_listener(proxy, implementation, data);
    }

    const char *class_name = real_wl_proxy_get_class(proxy);
    if (class_name == NULL ||
        strcmp(class_name, "zwp_linux_dmabuf_feedback_v1") != 0) {
        return real_wl_proxy_add_listener(proxy, implementation, data);
    }

    load_devt_mappings();
    if (devt_mapping_count == 0) {
        return real_wl_proxy_add_listener(proxy, implementation, data);
    }

    struct feedback_listener_state *state = calloc(1, sizeof(*state));
    if (state == NULL) {
        return real_wl_proxy_add_listener(proxy, implementation, data);
    }

    state->data = data;
    state->done = (feedback_done_fn)implementation[0];
    state->format_table = (feedback_format_table_fn)implementation[1];
    state->main_device = (feedback_main_device_fn)implementation[2];
    state->tranche_done = (feedback_tranche_done_fn)implementation[3];
    state->tranche_target_device =
        (feedback_tranche_target_device_fn)implementation[4];
    state->tranche_formats = (feedback_tranche_formats_fn)implementation[5];
    state->tranche_flags = (feedback_tranche_flags_fn)implementation[6];

    int result = real_wl_proxy_add_listener(proxy, feedback_listener, state);
    if (result != 0) {
        free(state);
    }
    return result;
}
