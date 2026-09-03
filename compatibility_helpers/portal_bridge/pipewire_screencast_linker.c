#include "pipewire_screencast_linker.h"
#include "portal_bridge_process.h"
#include <glob.h>
#include <spa/support/plugin.h>
#include <spa/utils/keys.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/time.h>
void pipewire_compat_try_links(PipeWireCompat *compat);
void publish_v4l2_cameras(PipeWireCompat *compat);

#define COMPAT_V4L2_CAP_VIDEO_CAPTURE UINT32_C(0x00000001)
#define COMPAT_V4L2_CAP_STREAMING UINT32_C(0x04000000)
#define COMPAT_V4L2_CAP_DEVICE_CAPS UINT32_C(0x80000000)
struct compat_v4l2_capability {
  uint8_t driver[16];
  uint8_t card[32];
  uint8_t bus_info[32];
  uint32_t version;
  uint32_t capabilities;
  uint32_t device_caps;
  uint32_t reserved[3];
};
#define COMPAT_VIDIOC_QUERYCAP _IOR('V', 0, struct compat_v4l2_capability)
struct compat_v4l2_format {
  uint8_t bytes[208];
};
#define COMPAT_VIDIOC_TRY_FMT _IOWR('V', 64, struct compat_v4l2_format)
struct compat_v4l2_buffer {
  uint32_t index;
  uint32_t type;
  uint32_t bytesused;
  uint32_t flags;
  uint32_t field;
  uint32_t alignment;
  struct timeval timestamp;
  uint8_t trailing[48];
};
_Static_assert(sizeof(struct compat_v4l2_buffer) == 88, "V4L2 buffer ABI");
#define COMPAT_VIDIOC_QBUF _IOWR('V', 15, struct compat_v4l2_buffer)
#define COMPAT_VIDIOC_DQBUF _IOWR('V', 17, struct compat_v4l2_buffer)

extern void *__sys_mmap(void *, size_t, int, int, int, off_t);
extern int __sys_open(const char *, int, mode_t);
extern int __sys_close(int);

static _Thread_local bool cache_v4l2_fd;
static _Thread_local int retained_v4l2_fd = -1;
static _Thread_local char retained_v4l2_path[64];
struct cached_v4l2_ioctl {
  struct compat_v4l2_format input;
  struct compat_v4l2_format output;
};
static _Thread_local struct cached_v4l2_ioctl cached_try_formats[32];
static _Thread_local size_t n_cached_try_formats;

extern int __sys_ioctl(int, unsigned long, char *);
bool pipewire_v4l2_mmap_retryable(int fd, int flags, int error) {
  char device[64];
  if (error != EINVAL || (flags & MAP_PRIVATE) == 0 ||
      (flags & MAP_SHARED) != 0 ||
      fdevname_r(fd, device, sizeof(device)) == NULL ||
      strncmp(device, "video", sizeof("video") - 1) != 0) {
    return false;
  }
  const char *suffix = device + sizeof("video") - 1;
  if (*suffix == 0) {
    return false;
  }
  while (*suffix >= 48 && *suffix <= 57) {
    suffix++;
  }
  return *suffix == 0;
}

void *compat_mmap(void *address, size_t length, int protection, int flags,
                  int fd, off_t offset) {
  void *result = __sys_mmap(address, length, protection, flags, fd, offset);
  int error = errno;
  if (result == MAP_FAILED && pipewire_v4l2_mmap_retryable(fd, flags, error)) {
    result = __sys_mmap(address, length, protection,
                        (flags & ~MAP_PRIVATE) | MAP_SHARED, fd, offset);
  } else {
    errno = error;
  }
  return result;
}
__sym_compat(mmap, compat_mmap, FBSD_1.0);

int compat_open(const char *path, int flags, ...) {
  mode_t mode = 0;
  if ((flags & O_CREAT) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, int);
    va_end(arguments);
  }
  if (cache_v4l2_fd && retained_v4l2_fd >= 0 &&
      g_strcmp0(path, retained_v4l2_path) == 0) {
    int fd = retained_v4l2_fd;
    retained_v4l2_fd = -1;
    return fd;
  }
  return __sys_open(path, flags, mode);
}
__sym_compat(open, compat_open, FBSD_1.0);

int compat_close(int fd) {
  char device[64];
  if (cache_v4l2_fd && retained_v4l2_fd < 0 &&
      fdevname_r(fd, device, sizeof(device)) != NULL &&
      g_snprintf(retained_v4l2_path, sizeof(retained_v4l2_path), "/dev/%s",
                 device) > 0 &&
      g_str_has_prefix(device, "video")) {
    retained_v4l2_fd = fd;
    return 0;
  }
  return __sys_close(fd);
}
__sym_compat(close, compat_close, FBSD_1.0);

static void begin_v4l2_fd_cache(void) {
  cache_v4l2_fd = true;
  n_cached_try_formats = 0;
}

static void end_v4l2_fd_cache(void) {
  cache_v4l2_fd = false;
  if (retained_v4l2_fd >= 0) {
    __sys_close(retained_v4l2_fd);
    retained_v4l2_fd = -1;
  }
  retained_v4l2_path[0] = 0;
  n_cached_try_formats = 0;
}

bool pipewire_v4l2_normalize_timestamp(int64_t timestamp_us,
                                       int64_t realtime_us,
                                       int64_t monotonic_us,
                                       int64_t *normalized_us) {
  const int64_t ten_minutes_us = INT64_C(10) * 60 * 1000 * 1000;
  const int64_t future_tolerance_us = INT64_C(60) * 1000 * 1000;
  if (normalized_us == NULL || timestamp_us < realtime_us - ten_minutes_us ||
      timestamp_us > realtime_us + future_tolerance_us) {
    return false;
  }
  *normalized_us = timestamp_us - (realtime_us - monotonic_us);
  return true;
}

bool pipewire_v4l2_ioctl_matches(unsigned long request,
                                 unsigned long expected) {
  return (uint32_t)request == (uint32_t)expected;
}

bool pipewire_v4l2_timestamp_is_stale(int64_t timestamp_us, int64_t realtime_us,
                                      int64_t monotonic_us,
                                      int64_t maximum_age_us) {
  int64_t normalized_us = 0;
  return maximum_age_us >= 0 &&
         pipewire_v4l2_normalize_timestamp(timestamp_us, realtime_us,
                                           monotonic_us, &normalized_us) &&
         monotonic_us - normalized_us > maximum_age_us;
}

int compat_ioctl(int fd, unsigned long request, ...) {
  static bool reported_first_dequeue;
  va_list arguments;
  va_start(arguments, request);
  void *argument = va_arg(arguments, void *);
  va_end(arguments);
  struct compat_v4l2_format try_format_input;
  bool cache_try_format =
      cache_v4l2_fd && argument != NULL &&
      pipewire_v4l2_ioctl_matches(request, COMPAT_VIDIOC_TRY_FMT);
  if (cache_try_format) {
    for (size_t i = 0; i < n_cached_try_formats; i++) {
      if (memcmp(argument, &cached_try_formats[i].input,
                 sizeof(struct compat_v4l2_format)) == 0) {
        memcpy(argument, &cached_try_formats[i].output,
               sizeof(struct compat_v4l2_format));
        return 0;
      }
    }
    memcpy(&try_format_input, argument, sizeof(try_format_input));
  }
  int result = __sys_ioctl(fd, request, argument);
  if (result == 0 && cache_v4l2_fd && argument != NULL &&
      n_cached_try_formats < G_N_ELEMENTS(cached_try_formats) &&
      cache_try_format) {
    struct cached_v4l2_ioctl *cached =
        &cached_try_formats[n_cached_try_formats++];
    memcpy(&cached->input, &try_format_input, sizeof(cached->input));
    memcpy(&cached->output, argument, sizeof(cached->output));
  }
  if (result == 0 &&
      pipewire_v4l2_ioctl_matches(request, COMPAT_VIDIOC_DQBUF) &&
      argument != NULL) {
    struct compat_v4l2_buffer *buffer = argument;
    struct compat_v4l2_buffer newest = *buffer;
    struct compat_v4l2_buffer dropped[16];
    size_t n_dropped = 0;
    while (n_dropped < G_N_ELEMENTS(dropped)) {
      struct compat_v4l2_buffer candidate = newest;
      if (__sys_ioctl(fd, COMPAT_VIDIOC_DQBUF, (char *)&candidate) != 0) {
        break;
      }
      dropped[n_dropped++] = newest;
      newest = candidate;
    }
    for (size_t i = 0; i < n_dropped; i++) {
      if (__sys_ioctl(fd, COMPAT_VIDIOC_QBUF, (char *)&dropped[i]) != 0) {
        log_line("requeue stale V4L2 frame failed: %s", g_strerror(errno));
      }
    }
    *buffer = newest;
    struct timespec realtime = {0};
    struct timespec monotonic = {0};
    if (clock_gettime(CLOCK_REALTIME, &realtime) == 0 &&
        clock_gettime(CLOCK_MONOTONIC, &monotonic) == 0) {
      int64_t timestamp_us = (int64_t)buffer->timestamp.tv_sec * 1000000 +
                             buffer->timestamp.tv_usec;
      int64_t realtime_us =
          (int64_t)realtime.tv_sec * 1000000 + realtime.tv_nsec / 1000;
      int64_t monotonic_us =
          (int64_t)monotonic.tv_sec * 1000000 + monotonic.tv_nsec / 1000;
      int64_t normalized_us = 0;
      if (pipewire_v4l2_normalize_timestamp(timestamp_us, realtime_us,
                                            monotonic_us, &normalized_us)) {
        if (pipewire_v4l2_timestamp_is_stale(timestamp_us, realtime_us,
                                             monotonic_us, INT64_C(150000))) {
          if (__sys_ioctl(fd, COMPAT_VIDIOC_QBUF, (char *)buffer) != 0) {
            log_line("requeue stale V4L2 frame failed: %s", g_strerror(errno));
          }
          errno = EAGAIN;
          return -1;
        }
        buffer->timestamp.tv_sec = normalized_us / 1000000;
        buffer->timestamp.tv_usec = normalized_us % 1000000;
        if (!reported_first_dequeue) {
          reported_first_dequeue = true;
          diagnostic_line("normalized first V4L2 frame timestamp (capture "
                          "age=%" G_GINT64_FORMAT " us)",
                          monotonic_us - normalized_us);
        }
      }
    }
  }
  return result;
}
__sym_compat(ioctl, compat_ioctl, FBSD_1.0);
uint32_t parse_pipewire_id(const char *value) {
  if (value == NULL || *value == '\0') {
    return SPA_ID_INVALID;
  }
  char *end = NULL;
  errno = 0;
  unsigned long parsed = strtoul(value, &end, 10);
  if (errno != 0 || end == value || *end != '\0' || parsed > UINT32_MAX) {
    return SPA_ID_INVALID;
  }
  return (uint32_t)parsed;
}

uint64_t parse_pipewire_serial(const char *value) {
  if (value == NULL || *value == '\0') {
    return 0;
  }
  char *end = NULL;
  errno = 0;
  unsigned long long parsed = strtoull(value, &end, 10);
  if (errno != 0 || end == value || *end != '\0') {
    return 0;
  }
  return (uint64_t)parsed;
}

ScreenCastSource *session_source_for_id(SessionRecord *session,
                                        uint32_t node_id) {
  if (session == NULL || session->sources == NULL) {
    return NULL;
  }
  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
    if (source->node_id == node_id) {
      return source;
    }
  }
  return NULL;
}

bool session_approves_source(SessionRecord *session, uint32_t node_id) {
  return session_source_for_id(session, node_id) != NULL;
}

bool source_generation_matches(const ScreenCastSource *source,
                               const PipeWireNode *node) {
  return source != NULL && node != NULL && source->node_id == node->id &&
         (source->serial == 0 || node->serial == 0 ||
          source->serial == node->serial);
}

void remove_session_source_for_node(SessionRecord *session,
                                    const PipeWireNode *node) {
  if (session == NULL || session->sources == NULL || node == NULL) {
    return;
  }
  for (guint i = session->sources->len; i > 0; i--) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i - 1);
    if (source_generation_matches(source, node)) {
      g_array_remove_index(session->sources, i - 1);
    }
  }
}

void free_pipewire_client(PipeWireClient *client) {
  if (client == NULL) {
    return;
  }
  if (client->proxy != NULL) {
    spa_hook_remove(&client->listener);
    pw_proxy_destroy((struct pw_proxy *)client->proxy);
  }
  if (client->permissions != NULL) {
    g_array_free(client->permissions, TRUE);
  }
  g_free(client);
}

void free_pipewire_node(PipeWireNode *node) {
  if (node == NULL) {
    return;
  }
  g_free(node->media_class);
  g_free(node->media_role);
  g_free(node->target_object);
  g_free(node);
}

void free_pipewire_port(PipeWirePort *port) { g_free(port); }

bool pipewire_node_is_camera(const PipeWireNode *node) {
  return node != NULL && g_strcmp0(node->media_class, "Video/Source") == 0 &&
         g_strcmp0(node->media_role, "Camera") == 0;
}

bool pipewire_camera_present(const PipeWireCompat *compat) {
  if (compat == NULL) {
    return false;
  }
  for (guint i = 0; i < compat->nodes->len; i++) {
    PipeWireNode *node = g_ptr_array_index(compat->nodes, i);
    if (!pipewire_node_is_camera(node)) {
      continue;
    }
    for (guint j = 0; j < compat->ports->len; j++) {
      PipeWirePort *port = g_ptr_array_index(compat->ports, j);
      if (port->node_id == node->id && port->is_output) {
        return true;
      }
    }
  }
  return false;
}
bool pipewire_camera_available(const PipeWireCompat *compat) {
  if (pipewire_camera_present(compat)) {
    return true;
  }

  glob_t devices = {0};
  bool available =
      glob("/dev/video[0-9]*", 0, NULL, &devices) == 0 && devices.gl_pathc > 0;
  globfree(&devices);
  return available;
}

bool pipewire_camera_publication_needed(const PipeWireCompat *compat) {
  return compat != NULL && compat->camera_requested &&
         !pipewire_camera_present(compat);
}

void free_pipewire_link(PipeWireLink *link) {
  if (link == NULL) {
    return;
  }
  if (link->proxy != NULL) {
    spa_hook_remove(&link->proxy_listener);
    pw_proxy_destroy(link->proxy);
  }
  g_free(link);
}

PipeWireNode *find_pipewire_node(PipeWireCompat *compat, uint32_t id) {
  for (guint i = 0; i < compat->nodes->len; i++) {
    PipeWireNode *node = g_ptr_array_index(compat->nodes, i);
    if (node->id == id) {
      return node;
    }
  }
  return NULL;
}

bool pipewire_client_permission(PipeWireClient *client, uint32_t object_id,
                                uint32_t *out_permissions) {
  for (guint i = 0; i < client->permissions->len; i++) {
    struct pw_permission *permission =
        &g_array_index(client->permissions, struct pw_permission, i);
    if (permission->id == object_id) {
      *out_permissions = permission->permissions;
      return true;
    }
  }
  *out_permissions = 0;
  return false;
}

bool pipewire_client_is_restricted(PipeWireClient *client) {
  uint32_t default_permissions = 0;
  return client->is_portal && client->permissions_received &&
         pipewire_client_permission(client, PW_ID_ANY, &default_permissions) &&
         default_permissions == 0;
}

bool pipewire_client_matches_session(PipeWireClient *client,
                                     SessionRecord *session) {
  if (!pipewire_client_is_restricted(client) || session == NULL ||
      session->closed || session->close_requested || session->sources == NULL ||
      session->sources->len == 0) {
    return false;
  }

  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
    uint32_t permissions = 0;
    if (!pipewire_client_permission(client, source->node_id, &permissions) ||
        (permissions & PW_PERM_R) == 0) {
      return false;
    }
  }

  BridgeState *state = session->state;
  for (guint i = 0; i < state->screencast.sessions->len; i++) {
    SessionRecord *other = g_ptr_array_index(state->screencast.sessions, i);
    if (other == session || other->sources == NULL) {
      continue;
    }
    for (guint j = 0; j < other->sources->len; j++) {
      ScreenCastSource *source =
          &g_array_index(other->sources, ScreenCastSource, j);
      uint32_t permissions = 0;
      if (!session_approves_source(session, source->node_id) &&
          pipewire_client_permission(client, source->node_id, &permissions) &&
          (permissions & PW_PERM_R) != 0) {
        return false;
      }
    }
  }
  return true;
}

bool source_node_is_approved(SessionRecord *session, PipeWireNode *node) {
  return source_generation_matches(session_source_for_id(session, node->id),
                                   node);
}

PipeWireNode *source_node_for_consumer(PipeWireCompat *compat,
                                       SessionRecord *session,
                                       PipeWireNode *consumer) {
  if (session->sources->len == 1) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, 0);
    PipeWireNode *node = find_pipewire_node(compat, source->node_id);
    return node != NULL && source_node_is_approved(session, node) ? node : NULL;
  }

  if (consumer->target_object == NULL) {
    return NULL;
  }
  uint64_t target = parse_pipewire_serial(consumer->target_object);
  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
    if (target != source->node_id &&
        (source->serial == 0 || target != source->serial)) {
      continue;
    }
    PipeWireNode *node = find_pipewire_node(compat, source->node_id);
    if (node != NULL && source_node_is_approved(session, node)) {
      return node;
    }
  }
  return NULL;
}

bool pipewire_link_exists(PipeWireCompat *compat, SessionRecord *session,
                          uint32_t source_port_id, uint32_t consumer_port_id) {
  for (guint i = 0; i < compat->links->len; i++) {
    PipeWireLink *link = g_ptr_array_index(compat->links, i);
    if (link->proxy != NULL && link->session == session &&
        link->source_port_id == source_port_id &&
        link->consumer_port_id == consumer_port_id) {
      return true;
    }
  }
  return false;
}

void on_pipewire_link_destroy(void *user_data) {
  PipeWireLink *link = user_data;
  spa_hook_remove(&link->proxy_listener);
  link->proxy = NULL;
}

void on_pipewire_link_removed(void *user_data) {
  PipeWireLink *link = user_data;
  PipeWireCompat *compat = link->compat;
  pw_proxy_destroy(link->proxy);
  g_ptr_array_remove(compat->links, link);
}

void on_pipewire_link_error(void *user_data, int seq, int result,
                            const char *message) {
  (void)seq;
  PipeWireLink *link = user_data;
  log_line("PipeWire compatibility link %u -> %u failed: %s (%s)",
           link->source_node_id, link->consumer_node_id, message,
           spa_strerror(result));
}

static const struct pw_proxy_events PIPEWIRE_LINK_PROXY_EVENTS = {
    PW_VERSION_PROXY_EVENTS,
    .destroy = on_pipewire_link_destroy,
    .removed = on_pipewire_link_removed,
    .error = on_pipewire_link_error,
};

void create_pipewire_link(PipeWireCompat *compat, SessionRecord *session,
                          PipeWireClient *client, PipeWireNode *source,
                          PipeWirePort *source_port, PipeWireNode *consumer,
                          PipeWirePort *consumer_port) {
  if (!session_approves_source(session, source->id) ||
      !source_node_is_approved(session, source) ||
      pipewire_link_exists(compat, session, source_port->id,
                           consumer_port->id)) {
    return;
  }

  char *source_node_id = g_strdup_printf("%u", source->id);
  char *source_port_id = g_strdup_printf("%u", source_port->id);
  char *consumer_node_id = g_strdup_printf("%u", consumer->id);
  char *consumer_port_id = g_strdup_printf("%u", consumer_port->id);
  struct pw_properties *properties = pw_properties_new(
      PW_KEY_LINK_OUTPUT_NODE, source_node_id, PW_KEY_LINK_OUTPUT_PORT,
      source_port_id, PW_KEY_LINK_INPUT_NODE, consumer_node_id,
      PW_KEY_LINK_INPUT_PORT, consumer_port_id, PW_KEY_OBJECT_LINGER, "false",
      NULL);
  struct pw_proxy *proxy = pw_core_create_object(
      compat->core, "link-factory", PW_TYPE_INTERFACE_Link, PW_VERSION_LINK,
      &properties->dict, 0);
  pw_properties_free(properties);
  g_free(source_node_id);
  g_free(source_port_id);
  g_free(consumer_node_id);
  g_free(consumer_port_id);
  if (proxy == NULL) {
    log_line("create PipeWire compatibility link %u -> %u failed: %s",
             source->id, consumer->id, g_strerror(errno));
    return;
  }

  PipeWireLink *link = g_new0(PipeWireLink, 1);
  link->compat = compat;
  link->session = session;
  link->proxy = proxy;
  link->source_node_id = source->id;
  link->source_port_id = source_port->id;
  link->consumer_client_id = client->id;
  link->consumer_node_id = consumer->id;
  link->consumer_port_id = consumer_port->id;
  pw_proxy_add_listener(link->proxy, &link->proxy_listener,
                        &PIPEWIRE_LINK_PROXY_EVENTS, link);
  g_ptr_array_add(compat->links, link);
  diagnostic_line(
      "linked approved ScreenCast source %u:%u -> portal client %u node %u:%u",
      source->id, source_port->id, client->id, consumer->id, consumer_port->id);
}

void pipewire_compat_try_links(PipeWireCompat *compat) {
  if (compat == NULL || compat->core == NULL) {
    return;
  }
  for (guint client_index = 0; client_index < compat->clients->len;
       client_index++) {
    PipeWireClient *client = g_ptr_array_index(compat->clients, client_index);
    for (guint session_index = 0;
         session_index < compat->state->screencast.sessions->len;
         session_index++) {
      SessionRecord *session =
          g_ptr_array_index(compat->state->screencast.sessions, session_index);
      if (!pipewire_client_matches_session(client, session)) {
        continue;
      }
      for (guint node_index = 0; node_index < compat->nodes->len;
           node_index++) {
        PipeWireNode *consumer = g_ptr_array_index(compat->nodes, node_index);
        if (consumer->client_id != client->id ||
            g_strcmp0(consumer->media_class, "Stream/Input/Video") != 0) {
          continue;
        }
        PipeWireNode *source =
            source_node_for_consumer(compat, session, consumer);
        if (source == NULL) {
          continue;
        }
        for (guint out_index = 0; out_index < compat->ports->len; out_index++) {
          PipeWirePort *output = g_ptr_array_index(compat->ports, out_index);
          if (!output->is_output || output->node_id != source->id) {
            continue;
          }
          for (guint in_index = 0; in_index < compat->ports->len; in_index++) {
            PipeWirePort *input = g_ptr_array_index(compat->ports, in_index);
            if (input->is_input && input->node_id == consumer->id) {
              create_pipewire_link(compat, session, client, source, output,
                                   consumer, input);
            }
          }
        }
      }
    }
  }
}

void remove_pipewire_links_for_session(SessionRecord *session) {
  PipeWireCompat *compat =
      session != NULL ? session->state->screencast.pipewire : NULL;
  if (compat == NULL) {
    return;
  }
  for (guint i = compat->links->len; i > 0; i--) {
    PipeWireLink *link = g_ptr_array_index(compat->links, i - 1);
    if (link->session == session) {
      g_ptr_array_remove_index(compat->links, i - 1);
    }
  }
}

void remove_pipewire_links_for_object(PipeWireCompat *compat,
                                      uint32_t object_id, bool client) {
  for (guint i = compat->links->len; i > 0; i--) {
    PipeWireLink *link = g_ptr_array_index(compat->links, i - 1);
    bool matches = client ? link->consumer_client_id == object_id
                          : link->source_node_id == object_id ||
                                link->consumer_node_id == object_id ||
                                link->source_port_id == object_id ||
                                link->consumer_port_id == object_id;
    if (matches) {
      g_ptr_array_remove_index(compat->links, i - 1);
    }
  }
}

void on_pipewire_client_permissions(void *user_data, uint32_t index,
                                    uint32_t n_permissions,
                                    const struct pw_permission *permissions) {
  PipeWireClient *client = user_data;
  if (index == 0) {
    g_array_set_size(client->permissions, 0);
  }
  if (index > client->permissions->len) {
    g_array_set_size(client->permissions, index);
  }
  for (uint32_t i = 0; i < n_permissions; i++) {
    if (index + i < client->permissions->len) {
      g_array_index(client->permissions, struct pw_permission, index + i) =
          permissions[i];
    } else {
      g_array_append_val(client->permissions, permissions[i]);
    }
  }
  client->permissions_received = true;
  pipewire_compat_try_links(client->compat);
}

static const struct pw_client_events PIPEWIRE_CLIENT_EVENTS = {
    PW_VERSION_CLIENT_EVENTS,
    .permissions = on_pipewire_client_permissions,
};

void refresh_pipewire_client_permissions(PipeWireClient *client) {
  if (client == NULL || client->proxy == NULL) {
    return;
  }
  int result = pw_client_get_permissions(client->proxy, 0, UINT32_MAX);
  if (result < 0) {
    log_line("read PipeWire portal client %u permissions failed: %s",
             client->id, spa_strerror(result));
  }
}

void refresh_pipewire_permissions_for_client(PipeWireCompat *compat,
                                             uint32_t client_id) {
  if (compat == NULL) {
    return;
  }
  for (guint i = 0; i < compat->clients->len; i++) {
    PipeWireClient *client = g_ptr_array_index(compat->clients, i);
    if (client_id == SPA_ID_INVALID || client->id == client_id) {
      refresh_pipewire_client_permissions(client);
    }
  }
}

void on_pipewire_registry_global(void *user_data, uint32_t id,
                                 uint32_t permissions, const char *type,
                                 uint32_t version,
                                 const struct spa_dict *properties) {
  (void)permissions;
  PipeWireCompat *compat = user_data;
  if (g_strcmp0(type, PW_TYPE_INTERFACE_Client) == 0) {
    const char *access = spa_dict_lookup(properties, "pipewire.access");
    if (g_strcmp0(access, "portal") != 0) {
      return;
    }
    PipeWireClient *client = g_new0(PipeWireClient, 1);
    client->compat = compat;
    client->id = id;
    client->is_portal = true;
    client->permissions =
        g_array_new(FALSE, TRUE, sizeof(struct pw_permission));
    client->proxy =
        pw_registry_bind(compat->registry, id, PW_TYPE_INTERFACE_Client,
                         SPA_MIN(version, PW_VERSION_CLIENT), 0);
    if (client->proxy == NULL) {
      free_pipewire_client(client);
      return;
    }
    pw_client_add_listener(client->proxy, &client->listener,
                           &PIPEWIRE_CLIENT_EVENTS, client);
    g_ptr_array_add(compat->clients, client);
    refresh_pipewire_client_permissions(client);
  } else if (g_strcmp0(type, PW_TYPE_INTERFACE_Node) == 0) {
    PipeWireNode *node = g_new0(PipeWireNode, 1);
    node->id = id;
    node->client_id =
        parse_pipewire_id(spa_dict_lookup(properties, PW_KEY_CLIENT_ID));
    node->serial = parse_pipewire_serial(
        spa_dict_lookup(properties, PW_KEY_OBJECT_SERIAL));
    node->media_class =
        g_strdup(spa_dict_lookup(properties, PW_KEY_MEDIA_CLASS));
    node->media_role = g_strdup(spa_dict_lookup(properties, PW_KEY_MEDIA_ROLE));
    node->target_object =
        g_strdup(spa_dict_lookup(properties, PW_KEY_TARGET_OBJECT));
    g_ptr_array_add(compat->nodes, node);
    if (pipewire_node_is_camera(node)) {
      diagnostic_line("discovered host PipeWire camera node %u (%s)", node->id,
                      node->media_role != NULL ? node->media_role : "no role");
    }
    if (g_strcmp0(node->media_class, "Stream/Input/Video") == 0) {
      refresh_pipewire_permissions_for_client(compat, node->client_id);
    }
    pipewire_compat_try_links(compat);
  } else if (g_strcmp0(type, PW_TYPE_INTERFACE_Port) == 0) {
    PipeWirePort *port = g_new0(PipeWirePort, 1);
    port->id = id;
    port->node_id =
        parse_pipewire_id(spa_dict_lookup(properties, PW_KEY_NODE_ID));
    const char *direction = spa_dict_lookup(properties, PW_KEY_PORT_DIRECTION);
    port->is_input = g_strcmp0(direction, "in") == 0;
    port->is_output = g_strcmp0(direction, "out") == 0;
    g_ptr_array_add(compat->ports, port);
    pipewire_compat_try_links(compat);
  }
}

void on_pipewire_registry_global_remove(void *user_data, uint32_t id) {
  PipeWireCompat *compat = user_data;
  bool camera_removed = false;
  for (guint i = compat->clients->len; i > 0; i--) {
    PipeWireClient *client = g_ptr_array_index(compat->clients, i - 1);
    if (client->id == id) {
      remove_pipewire_links_for_object(compat, id, true);
      g_ptr_array_remove_index(compat->clients, i - 1);
    }
  }
  for (guint i = compat->nodes->len; i > 0; i--) {
    PipeWireNode *node = g_ptr_array_index(compat->nodes, i - 1);
    if (node->id != id) {
      continue;
    }
    camera_removed = pipewire_node_is_camera(node);
    remove_pipewire_links_for_object(compat, id, false);
    for (guint session_index = 0;
         session_index < compat->state->screencast.sessions->len;
         session_index++) {
      SessionRecord *session =
          g_ptr_array_index(compat->state->screencast.sessions, session_index);
      if (session->sources == NULL) {
        continue;
      }
      remove_session_source_for_node(session, node);
    }
    g_ptr_array_remove_index(compat->nodes, i - 1);
  }
  for (guint i = compat->ports->len; i > 0; i--) {
    PipeWirePort *port = g_ptr_array_index(compat->ports, i - 1);
    if (port->id == id) {
      remove_pipewire_links_for_object(compat, id, false);
      g_ptr_array_remove_index(compat->ports, i - 1);
    }
  }
  if (camera_removed && pipewire_camera_publication_needed(compat)) {
    if (compat->camera_lock_fd >= 0) {
      g_ptr_array_set_size(compat->published_cameras, 0);
    }
    publish_v4l2_cameras(compat);
  }
}

static const struct pw_registry_events PIPEWIRE_REGISTRY_EVENTS = {
    PW_VERSION_REGISTRY_EVENTS,
    .global = on_pipewire_registry_global,
    .global_remove = on_pipewire_registry_global_remove,
};

void destroy_published_camera(gpointer data) {
  PublishedCamera *camera = data;
  if (camera == NULL) {
    return;
  }
  if (camera->proxy != NULL) {
    pw_proxy_destroy(camera->proxy);
  }
  if (camera->handle != NULL) {
    pw_unload_spa_handle(camera->handle);
  }
  g_free(camera);
}

bool pipewire_v4l2_caps_usable(uint32_t capabilities,
                               uint32_t device_capabilities) {
  uint32_t effective = (capabilities & COMPAT_V4L2_CAP_DEVICE_CAPS) != 0
                           ? device_capabilities
                           : capabilities;
  return (effective & COMPAT_V4L2_CAP_VIDEO_CAPTURE) != 0 &&
         (effective & COMPAT_V4L2_CAP_STREAMING) != 0;
}

static bool v4l2_camera_usable(const char *path) {
  int fd = open(path, O_RDWR | O_NONBLOCK | O_NOCTTY | O_CLOEXEC);
  if (fd < 0) {
    return false;
  }
  struct compat_v4l2_capability capability = {0};
  bool usable = ioctl(fd, COMPAT_VIDIOC_QUERYCAP, &capability) == 0 &&
                pipewire_v4l2_caps_usable(capability.capabilities,
                                          capability.device_caps);
  close(fd);
  return usable;
}

bool acquire_camera_publisher_lock(PipeWireCompat *compat) {
  if (compat->camera_lock_fd >= 0) {
    return true;
  }
  const char *runtime = g_get_user_runtime_dir();
  char *directory = g_build_filename(runtime, "freebsd-flatpak", NULL);
  if (g_mkdir_with_parents(directory, 0700) != 0) {
    log_line("create camera publisher runtime directory failed: %s",
             g_strerror(errno));
    g_free(directory);
    return false;
  }
  char *path = g_build_filename(directory, "camera-publisher.lock", NULL);
  g_free(directory);
  int fd = open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
  g_free(path);
  if (fd < 0 || flock(fd, LOCK_EX | LOCK_NB) != 0) {
    if (fd >= 0) {
      close(fd);
    }
    return false;
  }
  compat->camera_lock_fd = fd;
  return true;
}

void publish_v4l2_cameras(PipeWireCompat *compat) {
  int64_t started_us = g_get_monotonic_time();
  if (pipewire_camera_present(compat) ||
      !acquire_camera_publisher_lock(compat)) {
    return;
  }
  glob_t devices = {0};
  if (glob("/dev/video[0-9]*", 0, NULL, &devices) != 0) {
    globfree(&devices);
    return;
  }
  for (size_t i = 0; i < devices.gl_pathc; i++) {
    const char *path = devices.gl_pathv[i];
    if (!v4l2_camera_usable(path)) {
      diagnostic_line("skipping unusable V4L2 endpoint %s", path);
      continue;
    }
    char *element = g_path_get_basename(path);
    char *name = g_strdup_printf("freebsd_flatpak.camera.%s", element);
    char *description = g_strdup_printf("V4L2 Camera (%s)", path);
    struct pw_properties *properties = pw_properties_new(
        SPA_KEY_LIBRARY_NAME, "v4l2/libspa-v4l2", PW_KEY_FACTORY_NAME,
        "api.v4l2.source", SPA_KEY_API_V4L2_PATH, path, PW_KEY_NODE_NAME, name,
        PW_KEY_NODE_DESCRIPTION, description, PW_KEY_DEVICE_API, "v4l2",
        PW_KEY_MEDIA_CLASS, "Video/Source", PW_KEY_MEDIA_ROLE, "Camera",
        PW_KEY_NODE_PAUSE_ON_IDLE, "true", PW_KEY_OBJECT_LINGER, "false", NULL);
    struct spa_handle *handle = pw_context_load_spa_handle(
        compat->context, "api.v4l2.source", &properties->dict);
    struct spa_node *node = NULL;
    int interface_result =
        handle == NULL ? -errno
                       : spa_handle_get_interface(
                             handle, SPA_TYPE_INTERFACE_Node, (void **)&node);
    begin_v4l2_fd_cache();
    struct pw_proxy *proxy =
        interface_result < 0
            ? NULL
            : pw_core_export(compat->core, SPA_TYPE_INTERFACE_Node,
                             &properties->dict, node, 0);
    end_v4l2_fd_cache();
    pw_properties_free(properties);
    if (proxy == NULL) {
      int result = interface_result < 0 ? interface_result : -errno;
      log_line("publish V4L2 camera %s failed: %s", path, spa_strerror(result));
      if (handle != NULL) {
        pw_unload_spa_handle(handle);
      }
    } else {
      PublishedCamera *camera = g_new0(PublishedCamera, 1);
      camera->handle = handle;
      camera->proxy = proxy;
      g_ptr_array_add(compat->published_cameras, camera);
      diagnostic_line(
          "publishing V4L2 camera %s into host PipeWire after %" G_GINT64_FORMAT
          " us",
          path, g_get_monotonic_time() - started_us);
    }
    g_free(description);
    g_free(name);
    g_free(element);
  }
  globfree(&devices);
  if (compat->published_cameras->len == 0 && compat->camera_lock_fd >= 0) {
    close(compat->camera_lock_fd);
    compat->camera_lock_fd = -1;
  }
  pw_core_sync(compat->core, PW_ID_CORE, 0);
}

void pipewire_request_camera_publication(PipeWireCompat *compat) {
  if (compat == NULL) {
    return;
  }
  compat->camera_requested = true;
  publish_v4l2_cameras(compat);
}

void on_pipewire_core_error(void *user_data, uint32_t id, int seq, int result,
                            const char *message) {
  (void)user_data;
  (void)seq;
  log_line("PipeWire compatibility object %u failed: %s (%s)", id, message,
           spa_strerror(result));
}

static const struct pw_core_events PIPEWIRE_CORE_EVENTS = {
    PW_VERSION_CORE_EVENTS,
    .error = on_pipewire_core_error,
};

gboolean pipewire_source_prepare(GSource *source, gint *timeout) {
  (void)source;
  *timeout = -1;
  return FALSE;
}

gboolean pipewire_source_dispatch(GSource *source, GSourceFunc callback,
                                  gpointer user_data) {
  (void)callback;
  (void)user_data;
  PipeWireSource *pipewire_source = (PipeWireSource *)source;
  int result =
      pw_loop_iterate(pw_main_loop_get_loop(pipewire_source->compat->loop), 0);
  if (result < 0) {
    log_line("PipeWire compatibility loop failed: %s", spa_strerror(result));
  }
  return G_SOURCE_CONTINUE;
}

void pipewire_source_finalize(GSource *source) {
  PipeWireSource *pipewire_source = (PipeWireSource *)source;
  pw_loop_leave(pw_main_loop_get_loop(pipewire_source->compat->loop));
}

static GSourceFuncs PIPEWIRE_SOURCE_FUNCS = {
    .prepare = pipewire_source_prepare,
    .dispatch = pipewire_source_dispatch,
    .finalize = pipewire_source_finalize,
};

void free_pipewire_compat(PipeWireCompat *compat) {
  if (compat == NULL) {
    return;
  }
  if (compat->source != NULL) {
    g_source_destroy(compat->source);
    g_source_unref(compat->source);
  }
  if (compat->camera_lock_fd >= 0) {
    close(compat->camera_lock_fd);
    compat->camera_lock_fd = -1;
  }
  if (compat->published_cameras != NULL) {
    g_ptr_array_free(compat->published_cameras, TRUE);
  }
  if (compat->links != NULL) {
    g_ptr_array_free(compat->links, TRUE);
  }
  if (compat->clients != NULL) {
    g_ptr_array_free(compat->clients, TRUE);
  }
  if (compat->ports != NULL) {
    g_ptr_array_free(compat->ports, TRUE);
  }
  if (compat->nodes != NULL) {
    g_ptr_array_free(compat->nodes, TRUE);
  }
  if (compat->registry != NULL) {
    spa_hook_remove(&compat->registry_listener);
    pw_proxy_destroy((struct pw_proxy *)compat->registry);
  }
  if (compat->core != NULL) {
    spa_hook_remove(&compat->core_listener);
    pw_core_disconnect(compat->core);
  }
  if (compat->context != NULL) {
    pw_context_destroy(compat->context);
  }
  if (compat->loop != NULL) {
    pw_main_loop_destroy(compat->loop);
  }
  g_free(compat);
}

PipeWireCompat *new_pipewire_compat(BridgeState *state) {
  pw_init(NULL, NULL);
  PipeWireCompat *compat = g_new0(PipeWireCompat, 1);
  compat->state = state;
  compat->camera_lock_fd = -1;
  compat->clients =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_client);
  compat->nodes =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_node);
  compat->ports =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_port);
  compat->links =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_link);
  compat->published_cameras =
      g_ptr_array_new_with_free_func(destroy_published_camera);
  compat->loop = pw_main_loop_new(NULL);
  if (compat->loop == NULL) {
    free_pipewire_compat(compat);
    return NULL;
  }
  compat->context =
      pw_context_new(pw_main_loop_get_loop(compat->loop), NULL, 0);
  if (compat->context == NULL) {
    free_pipewire_compat(compat);
    return NULL;
  }
  struct pw_properties *properties = pw_properties_new(
      PW_KEY_APP_NAME, "freebsd-flatpak portal compatibility", NULL);
  compat->core = pw_context_connect(compat->context, properties, 0);
  if (compat->core == NULL) {
    free_pipewire_compat(compat);
    return NULL;
  }
  pw_core_add_listener(compat->core, &compat->core_listener,
                       &PIPEWIRE_CORE_EVENTS, compat);
  compat->registry = pw_core_get_registry(compat->core, PW_VERSION_REGISTRY, 0);
  if (compat->registry == NULL) {
    free_pipewire_compat(compat);
    return NULL;
  }
  pw_registry_add_listener(compat->registry, &compat->registry_listener,
                           &PIPEWIRE_REGISTRY_EVENTS, compat);

  PipeWireSource *source = (PipeWireSource *)g_source_new(
      &PIPEWIRE_SOURCE_FUNCS, sizeof(PipeWireSource));
  source->compat = compat;
  struct pw_loop *loop = pw_main_loop_get_loop(compat->loop);
  pw_loop_enter(loop);
  g_source_add_unix_fd(&source->source, pw_loop_get_fd(loop),
                       G_IO_IN | G_IO_ERR | G_IO_HUP);
  compat->source = &source->source;
  g_source_attach(compat->source, NULL);
  diagnostic_line("enabled ownership-based PipeWire ScreenCast linking");
  return compat;
}
