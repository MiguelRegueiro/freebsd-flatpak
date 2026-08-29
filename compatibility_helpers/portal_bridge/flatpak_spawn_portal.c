#include "flatpak_spawn_portal.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <arpa/inet.h>
#include <stddef.h>

#define SPAWN_MAX_PAYLOAD 4096
#define SPAWN_MAX_FDS 32
#define SPAWN_MAX_TARGET_FD 65535

/* The Rust broker owns sandbox/process execution. This helper is deliberately
 * limited to the D-Bus portal boundary and transports only validated data. */
const char FLATPAK_SPAWN_XML[] =
    "<node><interface name='org.freedesktop.portal.Flatpak'>"
    "<property name='version' type='u' access='read'/>"
    "<property name='supports' type='u' access='read'/>"
    "<method name='Spawn'><arg type='ay' direction='in' name='cwd'/>"
    "<arg type='aay' direction='in' name='argv'/><arg type='a{uh}' direction='in' name='fds'/><arg type='a{ss}' direction='in' name='envs'/><arg type='u' direction='in' name='flags'/><arg type='a{sv}' direction='in' name='options'/><arg type='u' direction='out' name='pid'/></method>"
    "<method name='SpawnSignal'><arg type='u' direction='in' name='pid'/>"
    "<arg type='u' direction='in' name='signal'/><arg type='b' direction='in' name='to_process_group'/></method>"
    "<signal name='SpawnExited'><arg type='u' name='pid'/><arg type='u' name='exit_status'/></signal><signal name='SpawnStarted'><arg type='u' name='pid'/><arg type='u' name='relpid'/></signal>"
    "</interface></node>";

static char *sender_root(BridgeState *state, const char *sender) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_sync(
      state->local_bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "GetConnectionUnixProcessID", g_variant_new("(s)", sender),
      G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  if (reply == NULL) { g_clear_error(&error); return NULL; }
  guint32 pid; g_variant_get(reply, "(u)", &pid); g_variant_unref(reply);
  gchar *pid_text = g_strdup_printf("%u", pid);
  gchar *argv[] = {"/usr/bin/procstat", "-f", pid_text, NULL};
  gchar *stdout_text = NULL;
  gboolean ok = g_spawn_sync(NULL, argv, NULL, G_SPAWN_STDERR_TO_DEV_NULL,
                             NULL, NULL, &stdout_text, NULL, NULL, &error);
  g_free(pid_text);
  if (!ok) { g_clear_error(&error); return NULL; }
  char **lines = g_strsplit(stdout_text, "\\n", -1); char *root = NULL;
  for (guint i = 0; lines[i] != NULL; i++) {
    char **fields = g_strsplit_set(lines[i], " \\t", -1); char *dense[64]; guint n = 0;
    for (guint j = 0; fields[j] != NULL && n < G_N_ELEMENTS(dense); j++) if (*fields[j]) dense[n++] = fields[j];
    for (guint j = 0; j < n; j++) {
      if (g_str_equal(dense[j], "root")) {
        root = g_strdup(dense[n - 1]);
        break;
      }
    }
    g_strfreev(fields); if (root) break;
  }
  g_strfreev(lines); g_free(stdout_text); return root;
}

/* The sender-root cache below handles callers that disconnect immediately.
 * This unique registration fallback is retained only for callers that connected
 * before the bridge observed their NameOwnerChanged signal. */
static char *unique_registered_root(BridgeState *state, guint *valid_count) {
  char *root = NULL;
  *valid_count = 0;
  gsize suffix_length = strlen(state->documents.mountpoint);
  for (guint i = 0; i < state->documents.sandbox_doc_dirs->len; i++) {
    const char *doc_dir = g_ptr_array_index(state->documents.sandbox_doc_dirs, i);
    gsize length = strlen(doc_dir);
    if (length <= suffix_length || !g_str_has_suffix(doc_dir, state->documents.mountpoint)) continue;
    char *candidate = g_strndup(doc_dir, length - suffix_length);
    char *info = g_build_filename(candidate, ".flatpak-info", NULL);
    gboolean valid = g_file_test(info, G_FILE_TEST_IS_REGULAR);
    g_free(info);
    if (!valid) {
      g_free(candidate);
      continue;
    }
    (*valid_count)++;
    if (root != NULL) { g_free(root); g_free(candidate); return NULL; }
    root = candidate;
  }
  return root;
}

#define SPAWN_SENDER_ROOT_CACHE_LIMIT 256
#define SPAWN_SENDER_ROOT_CACHE_TTL_USEC (30 * G_USEC_PER_SEC)

typedef struct {
  char *root;
  gint64 expires_at;
} SpawnSenderRoot;

void flatpak_spawn_sender_root_free(gpointer data) {
  SpawnSenderRoot *entry = data;
  if (entry == NULL) return;
  g_free(entry->root);
  g_free(entry);
}

static void prune_sender_root_cache(BridgeState *state, gint64 now) {
  if (state->spawn_sender_roots == NULL) return;
  GHashTableIter iter; gpointer key = NULL, value = NULL;
  g_hash_table_iter_init(&iter, state->spawn_sender_roots);
  while (g_hash_table_iter_next(&iter, &key, &value)) {
    SpawnSenderRoot *entry = value;
    if (entry->expires_at <= now) g_hash_table_iter_remove(&iter);
  }
}

static void cache_sender_root_value(BridgeState *state, const char *sender, char *root) {
  if (root == NULL) return;
  if (state->spawn_sender_roots == NULL || sender == NULL || sender[0] != ':') {
    g_free(root);
    return;
  }
  gint64 now = g_get_monotonic_time();
  prune_sender_root_cache(state, now);
  if (g_hash_table_size(state->spawn_sender_roots) >= SPAWN_SENDER_ROOT_CACHE_LIMIT &&
      !g_hash_table_contains(state->spawn_sender_roots, sender)) {
    g_free(root);
    return;
  }
  SpawnSenderRoot *entry = g_new0(SpawnSenderRoot, 1);
  entry->root = root;
  entry->expires_at = now + SPAWN_SENDER_ROOT_CACHE_TTL_USEC;
  g_hash_table_replace(state->spawn_sender_roots, g_strdup(sender), entry);
}

void flatpak_spawn_cache_sender_root(BridgeState *state, const char *sender) {
  cache_sender_root_value(state, sender, sender_root(state, sender));
}

static char *cached_sender_root(BridgeState *state, const char *sender) {
  if (state->spawn_sender_roots == NULL || sender == NULL) return NULL;
  gint64 now = g_get_monotonic_time();
  prune_sender_root_cache(state, now);
  SpawnSenderRoot *entry = g_hash_table_lookup(state->spawn_sender_roots, sender);
  return entry == NULL ? NULL : g_strdup(entry->root);
}

static bool broker_send_packet(int fd, guint16 type, guint32 request,
                               const void *payload, gsize payload_size,
                               const int *fds, gsize fd_count) {
  if (payload_size > SPAWN_MAX_PAYLOAD || fd_count > SPAWN_MAX_FDS) return false;
  unsigned char header[20] = {0}; guint32 magic = htonl(0x46534250), id = htonl(request), length = htonl((guint32)payload_size), count = htonl((guint32)fd_count); guint16 version = htons(1), message = htons(type);
  memcpy(header, &magic, 4); memcpy(header + 4, &version, 2); memcpy(header + 6, &message, 2); memcpy(header + 8, &id, 4); memcpy(header + 12, &length, 4); memcpy(header + 16, &count, 4);
  struct iovec iov[2] = {{ .iov_base = header, .iov_len = sizeof(header) }, { .iov_base = (void *)payload, .iov_len = payload_size }};
  char *control = NULL; struct msghdr msg = { .msg_iov = iov, .msg_iovlen = payload_size ? 2 : 1 };
  if (fd_count) { control = g_malloc0(CMSG_SPACE(fd_count * sizeof(int))); msg.msg_control = control; msg.msg_controllen = CMSG_SPACE(fd_count * sizeof(int)); struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg); cmsg->cmsg_level = SOL_SOCKET; cmsg->cmsg_type = SCM_RIGHTS; cmsg->cmsg_len = CMSG_LEN(fd_count * sizeof(int)); memcpy(CMSG_DATA(cmsg), fds, fd_count * sizeof(int)); }
  ssize_t sent; do { sent = sendmsg(fd, &msg, 0); } while (sent < 0 && errno == EINTR);
  g_free(control); return sent == (ssize_t)(sizeof(header) + payload_size);
}
typedef struct {
  BridgeState *state;
  int fd;
  guint source_id;
  guint32 request;
  guint32 pid;
  char *sender;
} SpawnLifecycle;

static void close_received_rights(struct msghdr *message) {
  for (struct cmsghdr *cmsg = CMSG_FIRSTHDR(message); cmsg != NULL;
       cmsg = CMSG_NXTHDR(message, cmsg)) {
    if (cmsg->cmsg_level != SOL_SOCKET || cmsg->cmsg_type != SCM_RIGHTS ||
        cmsg->cmsg_len < CMSG_LEN(0)) continue;
    gsize bytes = cmsg->cmsg_len - CMSG_LEN(0);
    if (bytes % sizeof(int) != 0) continue;
    int *rights = (int *)CMSG_DATA(cmsg);
    for (gsize i = 0; i < bytes / sizeof(int); i++) if (rights[i] >= 0) close(rights[i]);
  }
}

void flatpak_spawn_lifecycle_free(gpointer data) {
  SpawnLifecycle *lifecycle = data;

  if (lifecycle->source_id != 0) g_source_remove(lifecycle->source_id);
  if (lifecycle->fd >= 0) close(lifecycle->fd);
  g_free(lifecycle->sender);
  g_free(lifecycle);
}

static gboolean lifecycle_packet(int fd, guint16 *type, guint32 *request,
                                 const unsigned char **payload, gsize *length) {
  static unsigned char packet[20 + SPAWN_MAX_PAYLOAD];
  unsigned char control[CMSG_SPACE(SPAWN_MAX_FDS * sizeof(int))] = {0};
  struct iovec iov = { .iov_base = packet, .iov_len = sizeof(packet) };
  struct msghdr message = { .msg_iov = &iov, .msg_iovlen = 1, .msg_control = control, .msg_controllen = sizeof(control) };
  ssize_t count; do { count = recvmsg(fd, &message, MSG_DONTWAIT); } while (count < 0 && errno == EINTR);
  if (count <= 0) return FALSE;
  if (message.msg_flags & (MSG_TRUNC | MSG_CTRUNC) || CMSG_FIRSTHDR(&message) != NULL || count < 20) { close_received_rights(&message); return FALSE; }
  guint32 magic, wire_request, wire_length, fd_count; guint16 version, wire_type;
  memcpy(&magic, packet, 4); memcpy(&version, packet + 4, 2); memcpy(&wire_type, packet + 6, 2); memcpy(&wire_request, packet + 8, 4); memcpy(&wire_length, packet + 12, 4); memcpy(&fd_count, packet + 16, 4);
  *length = ntohl(wire_length);
  if (ntohl(magic) != 0x46534250 || ntohs(version) != 1 || ntohl(fd_count) != 0 || *length > SPAWN_MAX_PAYLOAD || count != (ssize_t)(20 + *length)) return FALSE;
  *type = ntohs(wire_type); *request = ntohl(wire_request); *payload = packet + 20; return TRUE;
}

static gboolean lifecycle_ready(gint fd, GIOCondition condition, gpointer data) {
  SpawnLifecycle *lifecycle = data;
  if (condition & G_IO_IN) {
    guint16 type; guint32 request; const unsigned char *payload; gsize length;
    if (lifecycle_packet(fd, &type, &request, &payload, &length)) {
      if (request == lifecycle->request && type == 6 && length == 4) {
        diagnostic_line("Flatpak broker lifecycle accepted request=%u", request);
        return G_SOURCE_CONTINUE;
      }
      if (request == lifecycle->request && type == 7 && length == 8) {
        guint32 status; memcpy(&status, payload + 4, 4);
        diagnostic_line("Flatpak broker lifecycle exited request=%u status=%u", request, ntohl(status));
      }
      if (request == lifecycle->request && type == 10 && length == 8) {
        guint32 pid, status;
        memcpy(&pid, payload, 4); memcpy(&status, payload + 4, 4);
        if (ntohl(pid) == lifecycle->pid) {
          GError *error = NULL;
          if (!g_dbus_connection_emit_signal(lifecycle->state->local_bus,
              lifecycle->sender, "/org/freedesktop/portal/Flatpak",
              "org.freedesktop.portal.Flatpak", "SpawnExited",
              g_variant_new("(uu)", lifecycle->pid, ntohl(status)), &error)) {
            log_line("emit Flatpak SpawnExited failed: %s", error->message);
            g_error_free(error);
          }
        }
      }
    }
  }
  lifecycle->source_id = 0;
  g_ptr_array_remove(lifecycle->state->spawn_lifecycles, lifecycle);
  return G_SOURCE_REMOVE;
}

void flatpak_spawn_watch_lifecycle(BridgeState *state, int fd, guint32 request,
                                   guint32 pid, const char *sender) {
  SpawnLifecycle *lifecycle = g_new0(SpawnLifecycle, 1);
  lifecycle->state = state; lifecycle->fd = fd; lifecycle->request = request;
  lifecycle->pid = pid; lifecycle->sender = g_strdup(sender);
  int flags = fcntl(fd, F_GETFL); if (flags >= 0) (void)fcntl(fd, F_SETFL, flags | O_NONBLOCK);
  lifecycle->source_id = g_unix_fd_add_full(G_PRIORITY_DEFAULT, fd, G_IO_IN | G_IO_HUP | G_IO_ERR | G_IO_NVAL, lifecycle_ready, lifecycle, NULL);
  g_ptr_array_add(state->spawn_lifecycles, lifecycle);
}

void flatpak_spawn_cleanup_lifecycles(BridgeState *state) {
  if (state->spawn_lifecycles != NULL) g_ptr_array_set_size(state->spawn_lifecycles, 0);
}

static bool broker_spawn_accepted(int fd, guint32 expected_request, guint32 *pid) {
  unsigned char response[24], control[CMSG_SPACE(SPAWN_MAX_FDS * sizeof(int))] = {0};
  struct iovec iov = { .iov_base = response, .iov_len = sizeof(response) };
  struct msghdr message = { .msg_iov = &iov, .msg_iovlen = 1, .msg_control = control, .msg_controllen = sizeof(control) };
  ssize_t received; do { received = recvmsg(fd, &message, 0); } while (received < 0 && errno == EINTR);
  if (received != sizeof(response) || message.msg_flags & (MSG_TRUNC | MSG_CTRUNC) || CMSG_FIRSTHDR(&message) != NULL) { close_received_rights(&message); return false; }
  guint32 magic, request, length, count, wire_pid; guint16 version, type;
  memcpy(&magic,response,4); memcpy(&version,response+4,2); memcpy(&type,response+6,2); memcpy(&request,response+8,4); memcpy(&length,response+12,4); memcpy(&count,response+16,4); memcpy(&wire_pid,response+20,4);
  if (ntohl(magic)!=0x46534250 || ntohs(version)!=1 || ntohs(type)!=9 || ntohl(request)!=expected_request || ntohl(length)!=4 || ntohl(count)!=0) return false;
  *pid = ntohl(wire_pid); return *pid != 0;
}

static bool path_is_descendant(const char *path, const char *parent) {
  gsize parent_length = strlen(parent);
  if (!g_str_has_prefix(path, parent) || parent_length == 0) return false;
  if (parent[parent_length - 1] == '/') return path[parent_length] != '\0';
  return path[parent_length] == '/';
}

static int broker_connect(BridgeState *state, const char *sender) {
  char *root = sender_root(state, sender);
  if (root != NULL) cache_sender_root_value(state, sender, g_strdup(root));
  if (root == NULL) root = cached_sender_root(state, sender);
  guint registered_instances = 0;
  if (root == NULL) root = unique_registered_root(state, &registered_instances);
  if (root == NULL) {
    diagnostic_line("Flatpak Spawn rejected: cannot resolve caller root (registered_instances=%u)", registered_instances);
    return -1;
  }
  char *chroots = g_path_get_dirname(state->documents.sandbox_root);
  char *canonical_root = realpath(root, NULL); char *canonical_chroots = realpath(chroots, NULL);
  g_free(root); g_free(chroots);
  if (!canonical_root || !canonical_chroots || !path_is_descendant(canonical_root, canonical_chroots)) { diagnostic_line("Flatpak Spawn rejected: caller root is outside sandbox instances"); free(canonical_root); free(canonical_chroots); return -1; }
  const char *relative = canonical_root + strlen(canonical_chroots); if (*relative == '/') relative++;
  char **parts = g_strsplit(relative, "/", 3); char *info = g_build_filename(canonical_root, ".flatpak-info", NULL);
  if (!parts[0] || !*parts[0] || !g_str_equal(parts[0], state->app_id) || !parts[1] || !*parts[1] || parts[2] || !g_file_test(info, G_FILE_TEST_IS_REGULAR)) { diagnostic_line("Flatpak Spawn rejected: caller root does not match portal app instance"); g_free(info); g_strfreev(parts); free(canonical_root); free(canonical_chroots); return -1; }
  g_free(info);
  char *app_dir = g_path_get_dirname(state->documents.doc_dir); char *apps = g_path_get_dirname(app_dir); char *portal = g_path_get_dirname(apps); char *runtime = g_path_get_dirname(portal);
  char *socket_path = g_strdup_printf("%s/spawn-brokers/%s.sock", runtime, parts[1]);
  g_free(app_dir); g_free(apps); g_free(portal); g_free(runtime); g_strfreev(parts); free(canonical_root); free(canonical_chroots);
  if (strlen(socket_path) >= sizeof(((struct sockaddr_un *)0)->sun_path)) { diagnostic_line("Flatpak Spawn rejected: broker socket path too long"); g_free(socket_path); return -1; }
  int fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0); if (fd < 0) { diagnostic_line("Flatpak Spawn rejected: cannot create broker socket: %s", g_strerror(errno)); g_free(socket_path); return -1; }
  struct sockaddr_un address = { .sun_family = AF_UNIX };
  g_strlcpy(address.sun_path, socket_path, sizeof(address.sun_path));
  socklen_t address_length = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(address.sun_path) + 1);
  address.sun_len = (unsigned char)address_length;
  g_free(socket_path);
  if (connect(fd, (struct sockaddr *)&address, address_length) != 0) { diagnostic_line("Flatpak Spawn rejected: broker connect failed: %s", g_strerror(errno)); close(fd); return -1; }
  return fd;
}

static void append_u32(GByteArray *payload, guint32 value) {
  value = htonl(value); g_byte_array_append(payload, (const guint8 *)&value, sizeof(value));
}

static bool append_byte_string(GByteArray *payload, GVariant *value, bool allow_empty, GError **error) {
  gsize length = 0; const guint8 *bytes = g_variant_get_fixed_array(value, &length, 1);
  if (length > 0 && bytes[length - 1] == 0) length--;
  if ((!allow_empty && length == 0) || memchr(bytes, 0, length) != NULL || length > G_MAXUINT32) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Invalid Spawn byte string"); return false;
  }
  append_u32(payload, (guint32)length); g_byte_array_append(payload, bytes, length); return true;
}

static void close_fd_array(GArray *fds) {
  for (guint i = 0; i < fds->len; i++) close(g_array_index(fds, int, i));
  g_array_free(fds, TRUE);
}

static bool build_spawn_payload(GVariant *cwd, GVariant *argv, GVariant *fd_map,
                                GVariant *envs, guint32 flags, GUnixFDList *fd_list,
                                GByteArray **out_payload, GArray **out_fds,
                                GError **error) {
  gsize argc = g_variant_n_children(argv), envc = g_variant_n_children(envs), mappings = g_variant_n_children(fd_map);
  if (argc == 0 || argc > 256 || envc > 256 || mappings > SPAWN_MAX_FDS || (mappings != 0 && fd_list == NULL)) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Invalid Spawn argument counts or fd list"); return false;
  }
  GByteArray *payload = g_byte_array_new(); GArray *fds = g_array_new(FALSE, FALSE, sizeof(int));
  bool ok = append_byte_string(payload, cwd, true, error);
  append_u32(payload, (guint32)argc); append_u32(payload, (guint32)envc); append_u32(payload, (guint32)mappings); append_u32(payload, flags);
  for (gsize index = 0; ok && index < argc; index++) {
    GVariant *argument = g_variant_get_child_value(argv, index); ok = append_byte_string(payload, argument, false, error); g_variant_unref(argument);
  }
  for (gsize index = 0; ok && index < envc; index++) {
    GVariant *entry = g_variant_get_child_value(envs, index); const char *key = NULL, *value = NULL; g_variant_get(entry, "{&s&s}", &key, &value);
    if (*key == 0 || strchr(key, '=') != NULL) { g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Invalid Spawn environment key"); ok = false; }
    if (ok) { append_u32(payload, strlen(key)); g_byte_array_append(payload, (const guint8 *)key, strlen(key)); append_u32(payload, strlen(value)); g_byte_array_append(payload, (const guint8 *)value, strlen(value)); }
    g_variant_unref(entry);
  }
  for (gsize index = 0; ok && index < mappings; index++) {
    GVariant *entry = g_variant_get_child_value(fd_map, index); guint32 target; gint32 handle; g_variant_get(entry, "{uh}", &target, &handle); g_variant_unref(entry);
    if (target > SPAWN_MAX_TARGET_FD) { g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Invalid Spawn target file descriptor"); ok = false; break; }
    for (guint previous = 0; previous < index; previous++) { guint32 seen; gint32 unused; GVariant *old = g_variant_get_child_value(fd_map, previous); g_variant_get(old, "{uh}", &seen, &unused); g_variant_unref(old); if (seen == target) { g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Duplicate Spawn target file descriptor"); ok = false; break; } }
    int source = ok ? g_unix_fd_list_get(fd_list, handle, error) : -1; if (source < 0) { ok = false; break; }
    g_array_append_val(fds, source); append_u32(payload, target);
  }
  if (ok && payload->len > SPAWN_MAX_PAYLOAD) { g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Spawn request is too large"); ok = false; }
  if (!ok) { g_byte_array_unref(payload); close_fd_array(fds); return false; }
  *out_payload = payload; *out_fds = fds; return true;
}

static bool validate_nested_options(guint32 flags, GVariant *options, GUnixFDList *fd_list, GError **error) {
  if (g_variant_n_children(options) == 0) return true;
  if ((flags & 0x04U) == 0) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS, "Spawn options require SANDBOX"); return false;
  }
  GVariantIter iterator; gchar *key = NULL; GVariant *value = NULL;
  g_variant_iter_init(&iterator, options);
  while (g_variant_iter_next(&iterator, "{sv}", &key, &value)) {
    gboolean valid = g_str_equal(key, "sandbox-expose-fd-ro") && g_variant_is_of_type(value, G_VARIANT_TYPE("ah"));
    if (!valid || fd_list == NULL) {
      g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_NOT_SUPPORTED, "Unsupported nested Spawn option: %s", key);
      g_free(key); g_variant_unref(value); return false;
    }
    GVariantIter handles; GVariant *handle = NULL;
    g_variant_iter_init(&handles, value);
    while ((handle = g_variant_iter_next_value(&handles)) != NULL) {
      gint fd = g_unix_fd_list_get(fd_list, g_variant_get_handle(handle), error);
      g_variant_unref(handle);
      if (fd < 0) { g_free(key); g_variant_unref(value); return false; }
      close(fd);
    }
    g_free(key); g_variant_unref(value);
  }
  return true;
}
static void call(GDBusConnection *c, const gchar *s, const gchar *p,
                 const gchar *i, const gchar *m, GVariant *v,
                 GDBusMethodInvocation *inv, gpointer data) {
  (void)c; (void)p; (void)i; BridgeState *state = data;
  if (g_str_equal(m, "SpawnSignal")) {
    g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.portal.Error.NotAllowed", "SpawnSignal is unavailable on this backend"); return;
  }
  GVariant *cwd = NULL, *argv = NULL, *fd_map = NULL, *envs = NULL, *options = NULL; guint32 flags;
  g_variant_get(v, "(@ay@aay@a{uh}@a{ss}u@a{sv})", &cwd, &argv, &fd_map, &envs, &flags, &options);
  diagnostic_line("Flatpak Spawn request=1 flags=%u cwd_bytes=%" G_GSIZE_FORMAT " argv_count=%" G_GSIZE_FORMAT " environment_count=%" G_GSIZE_FORMAT " fd_mappings=%" G_GSIZE_FORMAT " options=%" G_GSIZE_FORMAT, flags, g_variant_n_children(cwd), g_variant_n_children(argv), g_variant_n_children(envs), g_variant_n_children(fd_map), g_variant_n_children(options));
  if ((flags & ~0x1fU) != 0) {
    g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.portal.Error.NotAllowed", "Unsupported Flatpak Spawn flags");
    g_variant_unref(cwd); g_variant_unref(argv); g_variant_unref(fd_map); g_variant_unref(envs); g_variant_unref(options); return;
  }
  GError *option_error = NULL;
  if (!validate_nested_options(flags, options, g_dbus_message_get_unix_fd_list(g_dbus_method_invocation_get_message(inv)), &option_error)) {
    g_dbus_method_invocation_return_gerror(inv, option_error); g_error_free(option_error);
    g_variant_unref(cwd); g_variant_unref(argv); g_variant_unref(fd_map); g_variant_unref(envs); g_variant_unref(options); return;
  }
  GError *error = NULL; GByteArray *payload = NULL; GArray *fds = NULL;
  GDBusMessage *message = g_dbus_method_invocation_get_message(inv);
  if (!build_spawn_payload(cwd, argv, fd_map, envs, flags, g_dbus_message_get_unix_fd_list(message), &payload, &fds, &error)) {
    g_dbus_method_invocation_return_gerror(inv, error); g_error_free(error);
  } else {
    int fd = broker_connect(state, s); guint32 pid = 0;
    bool sent = fd >= 0 && broker_send_packet(fd, 8, 1, payload->data, payload->len, (const int *)fds->data, fds->len);
    bool accepted = sent && broker_spawn_accepted(fd, 1, &pid);
    if (!accepted) diagnostic_line("Flatpak Spawn broker rejection request=1 stage=%s payload_bytes=%" G_GSIZE_FORMAT " fd_mappings=%u", fd < 0 ? "connect" : (!sent ? "send" : "reply"), payload->len, fds->len);
    g_byte_array_unref(payload); close_fd_array(fds);
    if (!accepted) { if (fd >= 0) close(fd); g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.portal.Error.Failed", "Spawn broker rejected request"); }
    else { diagnostic_line("Flatpak Spawn accepted pid=%u", pid); flatpak_spawn_watch_lifecycle(state, fd, 1, pid, s); g_dbus_method_invocation_return_value(inv, g_variant_new("(u)", pid)); }
  }
  g_variant_unref(cwd); g_variant_unref(argv); g_variant_unref(fd_map); g_variant_unref(envs); g_variant_unref(options);
}
static GVariant *prop(GDBusConnection *c, const gchar *s, const gchar *p,
                      const gchar *i, const gchar *n, GError **e, gpointer d) {
  (void)c;(void)s;(void)p;(void)i;(void)e;(void)d;
  if (g_str_equal(n, "version")) return g_variant_new_uint32(4);
  if (g_str_equal(n, "supports")) return g_variant_new_uint32(0);
  g_set_error(e, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_PROPERTY, "Unknown property %s", n);
  return NULL;
}
const GDBusInterfaceVTable FLATPAK_SPAWN_VTABLE = {.method_call=call,.get_property=prop};
