#include <errno.h>
#include <fcntl.h>
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <glib/gstdio.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/user.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *DESKTOP_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.FileChooser'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='OpenFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFiles'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Settings'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Read'>"
    "      <arg type='s' name='namespace' direction='in'/>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='v' name='value' direction='out'/>"
    "    </method>"
    "    <method name='ReadAll'>"
    "      <arg type='as' name='namespaces' direction='in'/>"
    "      <arg type='a{sa{sv}}' name='values' direction='out'/>"
    "    </method>"
    "    <signal name='SettingChanged'>"
    "      <arg type='s' name='namespace'/>"
    "      <arg type='s' name='key'/>"
    "      <arg type='v' name='value'/>"
    "    </signal>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.ProxyResolver'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Lookup'>"
    "      <arg type='s' name='uri' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='as' name='proxies' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Inhibit'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Inhibit'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='CreateMonitor'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='QueryEndResponse'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "    </method>"
    "    <signal name='StateChanged'>"
    "      <arg type='o' name='session_handle'/>"
    "      <arg type='a{sv}' name='state'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

static const char *DOCUMENTS_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Documents'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='GetMountPoint'>"
    "      <arg type='ay' name='path' direction='out'/>"
    "    </method>"
    "    <method name='Add'>"
    "      <arg type='h' name='o_path_fd' direction='in'/>"
    "      <arg type='b' name='reuse_existing' direction='in'/>"
    "      <arg type='b' name='persistent' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='AddNamed'>"
    "      <arg type='h' name='o_path_parent_fd' direction='in'/>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='b' name='reuse_existing' direction='in'/>"
    "      <arg type='b' name='persistent' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='AddFull'>"
    "      <arg type='ah' name='o_path_fds' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "      <arg type='as' name='doc_ids' direction='out'/>"
    "      <arg type='a{sv}' name='extra_out' direction='out'/>"
    "    </method>"
    "    <method name='AddNamedFull'>"
    "      <arg type='h' name='o_path_fd' direction='in'/>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "      <arg type='a{sv}' name='extra_out' direction='out'/>"
    "    </method>"
    "    <method name='GrantPermissions'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "    </method>"
    "    <method name='RevokePermissions'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "    </method>"
    "    <method name='Delete'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "    </method>"
    "    <method name='Lookup'>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='Info'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='ay' name='path' direction='out'/>"
    "      <arg type='a{sas}' name='apps' direction='out'/>"
    "    </method>"
    "    <method name='List'>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='a{say}' name='docs' direction='out'/>"
    "    </method>"
    "    <method name='GetHostPaths'>"
    "      <arg type='as' name='doc_ids' direction='in'/>"
    "      <arg type='a{say}' name='paths' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.FileTransfer'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='StartTransfer'>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='s' name='key' direction='out'/>"
    "    </method>"
    "    <method name='AddFiles'>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='ah' name='fds' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "    </method>"
    "    <method name='RetrieveFiles'>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='as' name='files' direction='out'/>"
    "    </method>"
    "    <method name='StopTransfer'>"
    "      <arg type='s' name='key' direction='in'/>"
    "    </method>"
    "  </interface>"
    "</node>";

static const char *REQUEST_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Request'>"
    "    <method name='Close'/>"
    "    <signal name='Response'>"
    "      <arg type='u' name='response'/>"
    "      <arg type='a{sv}' name='results'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

typedef struct {
    char *doc_id;
    char *host_path;
    char *placeholder_path;
    char *target_path;
    char *app_id;
    char **permissions;
} DocumentGrant;

typedef struct _BridgeState BridgeState;

typedef struct {
    BridgeState *state;
    char *client_sender;
    char *local_path;
    guint local_registration_id;
    guint host_signal_id;
    bool completed;
} RequestRecord;

struct _BridgeState {
    char *app_id;
    char *doc_dir;
    char *sandbox_doc_dir;
    char *mountpoint;
    GPtrArray *grants;
    GPtrArray *requests;
    guint64 counter;
    guint64 request_counter;
    GMainLoop *loop;
    GDBusConnection *host_bus;
    GDBusConnection *local_bus;
    GDBusNodeInfo *desktop_node;
    GDBusNodeInfo *documents_node;
    GDBusNodeInfo *request_node;
    bool local_objects_registered;
};

static void log_line(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    fputs("portal bridge: ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
}

static GVariant *path_bytes_variant(const char *path)
{
    gsize len = strlen(path) + 1;
    return g_variant_new_fixed_array(G_VARIANT_TYPE_BYTE, path, len, sizeof(guchar));
}

static char **read_permissions(void)
{
    char **permissions = g_new0(char *, 2);
    permissions[0] = g_strdup("read");
    return permissions;
}

static void free_grant(DocumentGrant *grant)
{
    if (grant == NULL) {
        return;
    }
    g_free(grant->doc_id);
    g_free(grant->host_path);
    g_free(grant->placeholder_path);
    g_free(grant->target_path);
    g_free(grant->app_id);
    g_strfreev(grant->permissions);
    g_free(grant);
}

static bool run_argv(char **argv, GError **error)
{
    gint status = 0;
    gchar *stderr_text = NULL;
    if (!g_spawn_sync(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL, NULL, &stderr_text,
                      &status, error)) {
        g_free(stderr_text);
        return false;
    }
    if (!g_spawn_check_wait_status(status, error)) {
        if (stderr_text != NULL && *stderr_text != '\0') {
            log_line("%s", stderr_text);
        }
        g_free(stderr_text);
        return false;
    }
    g_free(stderr_text);
    return true;
}

static bool mount_file_read_only(const char *source, const char *target, GError **error)
{
    char *argv[] = { "doas", "mount_nullfs", "-o", "ro", (char *)source, (char *)target, NULL };
    return run_argv(argv, error);
}

static bool unmount_path(const char *target)
{
    GError *error = NULL;
    char *argv[] = { "doas", "umount", (char *)target, NULL };
    if (run_argv(argv, &error)) {
        return true;
    }
    if (error != NULL) {
        log_line("umount failed for %s: %s", target, error->message);
        g_error_free(error);
    }

    error = NULL;
    char *force_argv[] = { "doas", "umount", "-f", (char *)target, NULL };
    if (run_argv(force_argv, &error)) {
        return true;
    }
    if (error != NULL) {
        log_line("forced umount failed for %s: %s", target, error->message);
        g_error_free(error);
    }
    return false;
}

static void cleanup_grant(DocumentGrant *grant)
{
    if (grant == NULL || grant->target_path == NULL) {
        return;
    }
    unmount_path(grant->target_path);
    const char *placeholder =
        grant->placeholder_path != NULL ? grant->placeholder_path : grant->target_path;
    if (g_remove(placeholder) != 0 && errno != ENOENT) {
        log_line("remove %s failed: %s", placeholder, g_strerror(errno));
    }
    char *dir = g_path_get_dirname(placeholder);
    if (g_rmdir(dir) != 0 && errno != ENOENT) {
        log_line("remove %s failed: %s", dir, g_strerror(errno));
    }
    g_free(dir);
}

static void free_request(RequestRecord *request)
{
    if (request == NULL) {
        return;
    }
    if (request->host_signal_id != 0 && request->state->host_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus, request->host_signal_id);
    }
    if (request->local_registration_id != 0 && request->state->local_bus != NULL) {
        g_dbus_connection_unregister_object(request->state->local_bus,
                                            request->local_registration_id);
    }
    g_free(request->client_sender);
    g_free(request->local_path);
    g_free(request);
}

static void cleanup_all(BridgeState *state)
{
    for (guint i = 0; i < state->grants->len; i++) {
        cleanup_grant(g_ptr_array_index(state->grants, i));
    }
    g_ptr_array_set_size(state->grants, 0);
}

static gboolean handle_signal(gpointer user_data)
{
    BridgeState *state = user_data;
    cleanup_all(state);
    if (state->loop != NULL) {
        g_main_loop_quit(state->loop);
    }
    return G_SOURCE_REMOVE;
}

static bool fd_host_path(int fd, char *path, size_t path_len, GError **error)
{
    struct kinfo_file info;
    memset(&info, 0, sizeof(info));
    info.kf_structsize = sizeof(info);
    if (fcntl(fd, F_KINFO, &info) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "F_KINFO failed: %s",
                    g_strerror(errno));
        return false;
    }
    if (info.kf_type != KF_TYPE_VNODE || info.kf_un.kf_file.kf_file_type != KF_VTYPE_VREG ||
        info.kf_path[0] == '\0') {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                    "selected FD is not a regular path-backed file");
        return false;
    }
    g_strlcpy(path, info.kf_path, path_len);
    return true;
}

static char *safe_doc_id(BridgeState *state)
{
    GString *id = g_string_new("freebsd_flatpak_poc_");
    for (const char *p = state->app_id; *p != '\0'; p++) {
        if (g_ascii_isalnum(*p)) {
            g_string_append_c(id, *p);
        } else {
            g_string_append_c(id, '_');
        }
    }
    g_string_append_printf(id, "_%" G_GUINT64_FORMAT, ++state->counter);
    return g_string_free(id, FALSE);
}

static char *safe_path_element(const char *input)
{
    GString *value = g_string_new("");
    for (const char *p = input; p != NULL && *p != '\0'; p++) {
        if (g_ascii_isalnum(*p) || *p == '_') {
            g_string_append_c(value, *p);
        } else {
            g_string_append_c(value, '_');
        }
    }
    if (value->len == 0) {
        g_string_append(value, "x");
    }
    return g_string_free(value, FALSE);
}

static char **permissions_from_variant(GVariant *permissions)
{
    GPtrArray *items = g_ptr_array_new_with_free_func(g_free);
    GVariantIter iter;
    const char *permission = NULL;
    g_variant_iter_init(&iter, permissions);
    while (g_variant_iter_next(&iter, "&s", &permission)) {
        g_ptr_array_add(items, g_strdup(permission));
    }
    if (items->len == 0) {
        g_ptr_array_add(items, g_strdup("read"));
    }
    g_ptr_array_add(items, NULL);
    return (char **)g_ptr_array_free(items, FALSE);
}

static bool create_document_grant_from_path(BridgeState *state, const char *host_path,
                                            const char *app_id, char **permissions,
                                            DocumentGrant **out, GError **error)
{
    struct stat st;
    if (g_stat(host_path, &st) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "stat %s failed: %s",
                    host_path, g_strerror(errno));
        return false;
    }
    if (!S_ISREG(st.st_mode)) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_REGULAR_FILE,
                    "FileChooser V1 only grants regular files: %s", host_path);
        return false;
    }

    char *base = g_path_get_basename(host_path);
    char *doc_id = safe_doc_id(state);
    char *source_doc_dir = g_build_filename(state->doc_dir, doc_id, NULL);
    if (g_mkdir_with_parents(source_doc_dir, 0700) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "create %s failed: %s",
                    source_doc_dir, g_strerror(errno));
        g_free(base);
        g_free(doc_id);
        g_free(source_doc_dir);
        return false;
    }

    char *placeholder = g_build_filename(source_doc_dir, base, NULL);
    int placeholder_fd = g_open(placeholder, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    if (placeholder_fd < 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "create %s failed: %s",
                    placeholder, g_strerror(errno));
        g_free(base);
        g_free(doc_id);
        g_free(source_doc_dir);
        g_free(placeholder);
        return false;
    }
    close(placeholder_fd);

    char *sandbox_doc_dir = g_build_filename(state->sandbox_doc_dir, doc_id, NULL);
    char *target = g_build_filename(sandbox_doc_dir, base, NULL);

    if (!mount_file_read_only(host_path, target, error)) {
        g_remove(placeholder);
        g_rmdir(source_doc_dir);
        g_free(base);
        g_free(doc_id);
        g_free(source_doc_dir);
        g_free(sandbox_doc_dir);
        g_free(placeholder);
        g_free(target);
        return false;
    }

    DocumentGrant *grant = g_new0(DocumentGrant, 1);
    grant->doc_id = doc_id;
    grant->host_path = g_strdup(host_path);
    grant->placeholder_path = placeholder;
    grant->target_path = target;
    grant->app_id = g_strdup(app_id != NULL && *app_id != '\0' ? app_id : state->app_id);
    grant->permissions = permissions != NULL ? g_strdupv(permissions) : read_permissions();
    *out = grant;

    log_line("%s -> %s as %s/%s", grant->host_path, grant->target_path, grant->doc_id, base);
    g_free(base);
    g_free(source_doc_dir);
    g_free(sandbox_doc_dir);
    return true;
}

static bool create_document_grant_from_fd(BridgeState *state, int fd, const char *app_id,
                                          GVariant *permissions, DocumentGrant **out,
                                          GError **error)
{
    char host_path[PATH_MAX];
    if (!fd_host_path(fd, host_path, sizeof(host_path), error)) {
        return false;
    }
    char **permission_list = permissions_from_variant(permissions);
    bool ok = create_document_grant_from_path(state, host_path, app_id, permission_list, out,
                                              error);
    g_strfreev(permission_list);
    return ok;
}

static DocumentGrant *find_grant(BridgeState *state, const char *doc_id)
{
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        if (g_strcmp0(grant->doc_id, doc_id) == 0) {
            return grant;
        }
    }
    return NULL;
}

static void add_mountpoint_extra(BridgeState *state, GVariantBuilder *extra)
{
    g_variant_builder_add(extra, "{sv}", "mountpoint", path_bytes_variant(state->mountpoint));
}

static char *sandbox_uri_for_grant(BridgeState *state, DocumentGrant *grant)
{
    char *base = g_path_get_basename(grant->target_path);
    char *sandbox_path = g_build_filename(state->mountpoint, grant->doc_id, base, NULL);
    GError *error = NULL;
    char *uri = g_filename_to_uri(sandbox_path, NULL, &error);
    if (uri == NULL) {
        log_line("could not encode %s as URI: %s", sandbox_path, error->message);
        g_error_free(error);
    }
    g_free(base);
    g_free(sandbox_path);
    return uri;
}

static char *rewrite_file_uri(BridgeState *state, const char *uri)
{
    GError *error = NULL;
    char *host_path = g_filename_from_uri(uri, NULL, &error);
    if (host_path == NULL) {
        log_line("could not decode FileChooser URI %s: %s", uri, error->message);
        g_error_free(error);
        return NULL;
    }

    char **permissions = read_permissions();
    DocumentGrant *grant = NULL;
    if (!create_document_grant_from_path(state, host_path, state->app_id, permissions, &grant,
                                         &error)) {
        log_line("could not grant %s: %s", host_path, error->message);
        g_error_free(error);
        g_strfreev(permissions);
        g_free(host_path);
        return NULL;
    }
    g_strfreev(permissions);
    g_free(host_path);
    g_ptr_array_add(state->grants, grant);

    char *rewritten = sandbox_uri_for_grant(state, grant);
    if (rewritten != NULL) {
        log_line("rewrote FileChooser URI to %s", rewritten);
    }
    return rewritten;
}

static GVariant *rewrite_uri_array(BridgeState *state, GVariant *uris)
{
    GVariantBuilder rewritten;
    g_variant_builder_init(&rewritten, G_VARIANT_TYPE("as"));

    GVariantIter iter;
    const char *uri = NULL;
    g_variant_iter_init(&iter, uris);
    while (g_variant_iter_next(&iter, "&s", &uri)) {
        char *mapped = NULL;
        if (g_str_has_prefix(uri, "file://")) {
            mapped = rewrite_file_uri(state, uri);
        }
        g_variant_builder_add(&rewritten, "s", mapped != NULL ? mapped : uri);
        g_free(mapped);
    }

    return g_variant_builder_end(&rewritten);
}

static GVariant *rewrite_filechooser_results(BridgeState *state, guint32 response,
                                             GVariant *results)
{
    GVariantBuilder out;
    g_variant_builder_init(&out, G_VARIANT_TYPE("a{sv}"));

    GVariantIter iter;
    const char *key = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, results);
    while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
        if (response == 0 && g_strcmp0(key, "uris") == 0 &&
            g_variant_is_of_type(value, G_VARIANT_TYPE("as"))) {
            GVariant *rewritten = rewrite_uri_array(state, value);
            g_variant_builder_add(&out, "{sv}", key, rewritten);
        } else {
            g_variant_builder_add(&out, "{sv}", key, value);
        }
        g_variant_unref(value);
    }

    return g_variant_builder_end(&out);
}

static RequestRecord *find_request(BridgeState *state, const char *local_path)
{
    for (guint i = 0; i < state->requests->len; i++) {
        RequestRecord *request = g_ptr_array_index(state->requests, i);
        if (g_strcmp0(request->local_path, local_path) == 0) {
            return request;
        }
    }
    return NULL;
}

static void emit_request_response(RequestRecord *request, guint32 response, GVariant *results)
{
    if (request->completed) {
        return;
    }
    request->completed = true;
    GError *error = NULL;
    if (!g_dbus_connection_emit_signal(request->state->local_bus, request->client_sender,
                                       request->local_path,
                                       "org.freedesktop.portal.Request", "Response",
                                       g_variant_new("(u@a{sv})", response, results), &error)) {
        log_line("emit Response to %s failed: %s", request->client_sender, error->message);
        g_error_free(error);
        return;
    }
    log_line("emitted Response %u to %s on %s", response, request->client_sender,
             request->local_path);
}

static void emit_cancel_response(RequestRecord *request)
{
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
    emit_request_response(request, 2, g_variant_builder_end(&results));
}

static void on_host_response(GDBusConnection *connection, const gchar *sender_name,
                             const gchar *object_path, const gchar *interface_name,
                             const gchar *signal_name, GVariant *parameters,
                             gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    (void)interface_name;
    (void)signal_name;

    RequestRecord *request = user_data;
    guint32 response = 2;
    GVariant *results = NULL;
    g_variant_get(parameters, "(u@a{sv})", &response, &results);
    GVariant *rewritten = rewrite_filechooser_results(request->state, response, results);
    g_variant_unref(results);
    emit_request_response(request, response, rewritten);

    if (request->host_signal_id != 0) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus, request->host_signal_id);
        request->host_signal_id = 0;
    }
}

static void on_host_filechooser_call(GObject *source_object, GAsyncResult *result,
                                     gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    RequestRecord *request = user_data;
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
    if (reply == NULL) {
        log_line("host FileChooser call failed: %s", error->message);
        g_error_free(error);
        emit_cancel_response(request);
        return;
    }

    const char *host_handle = NULL;
    g_variant_get(reply, "(&o)", &host_handle);
    request->host_signal_id = g_dbus_connection_signal_subscribe(
        request->state->host_bus, "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Request", "Response", host_handle, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, on_host_response, request, NULL);
    g_variant_unref(reply);
}

static GVariant *option_value(GVariant *options, const char *key)
{
    GVariantIter iter;
    const char *name = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, options);
    while (g_variant_iter_next(&iter, "{&sv}", &name, &value)) {
        if (g_strcmp0(name, key) == 0) {
            return value;
        }
        g_variant_unref(value);
    }
    return NULL;
}

static char *request_path_for_call(BridgeState *state, const char *sender, GVariant *parameters)
{
    GVariant *options = g_variant_get_child_value(parameters, 2);
    GVariant *token_value = option_value(options, "handle_token");
    char *sender_element = safe_path_element(sender);
    char *token_element = NULL;
    if (token_value != NULL && g_variant_is_of_type(token_value, G_VARIANT_TYPE_STRING)) {
        token_element = safe_path_element(g_variant_get_string(token_value, NULL));
        g_variant_unref(token_value);
    } else {
        token_element = g_strdup_printf("freebsd_flatpak_poc_%" G_GUINT64_FORMAT,
                                        ++state->request_counter);
    }
    g_variant_unref(options);

    char *path = g_strdup_printf("/org/freedesktop/portal/desktop/request/%s/%s",
                                 sender_element, token_element);
    g_free(sender_element);
    g_free(token_element);
    return path;
}

static void handle_request_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)interface_name;
    (void)parameters;

    BridgeState *state = user_data;
    RequestRecord *request = find_request(state, object_path);
    if (request == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                              "unknown request object");
        return;
    }
    if (g_strcmp0(method_name, "Close") == 0) {
        emit_cancel_response(request);
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                          "%s is not implemented", method_name);
}

static GVariant *handle_request_property(GDBusConnection *connection, const gchar *sender,
                                         const gchar *object_path,
                                         const gchar *interface_name,
                                         const gchar *property_name, GError **error,
                                         gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)user_data;
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s", property_name);
    return NULL;
}

static const GDBusInterfaceVTable REQUEST_VTABLE = {
    .method_call = handle_request_method,
    .get_property = handle_request_property,
};

static void handle_filechooser_open(BridgeState *state, const char *sender,
                                    GVariant *parameters,
                                    GDBusMethodInvocation *invocation)
{
    GError *error = NULL;
    char *local_path = request_path_for_call(state, sender, parameters);
    GDBusInterfaceInfo *request_iface =
        g_dbus_node_info_lookup_interface(state->request_node,
                                          "org.freedesktop.portal.Request");
    RequestRecord *request = g_new0(RequestRecord, 1);
    request->state = state;
    request->client_sender = g_strdup(sender);
    request->local_path = g_strdup(local_path);
    request->local_registration_id = g_dbus_connection_register_object(
        state->local_bus, local_path, request_iface, &REQUEST_VTABLE, state, NULL, &error);
    if (request->local_registration_id == 0) {
        g_dbus_method_invocation_take_error(invocation, error);
        g_free(request->client_sender);
        g_free(request->local_path);
        g_free(request);
        g_free(local_path);
        return;
    }
    g_ptr_array_add(state->requests, request);

    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop",
                           "org.freedesktop.portal.FileChooser", "OpenFile", parameters,
                           G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           on_host_filechooser_call, request);

    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", local_path));
    log_line("forwarded FileChooser.OpenFile as %s", local_path);
    g_free(local_path);
}

static void on_forward_call(GObject *source_object, GAsyncResult *result, gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    GDBusMethodInvocation *invocation = user_data;
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
    if (reply == NULL) {
        g_dbus_method_invocation_take_error(invocation, error);
    } else {
        g_dbus_method_invocation_return_value(invocation, reply);
    }
    g_object_unref(invocation);
}

static char *fresh_request_path(BridgeState *state, const char *label)
{
    return g_strdup_printf("/org/freedesktop/portal/desktop/request/freebsd_flatpak_poc/%s_%" G_GUINT64_FORMAT,
                           label, ++state->request_counter);
}

static void return_immediate_empty_request(BridgeState *state, GDBusMethodInvocation *invocation,
                                           const char *label)
{
    char *path = fresh_request_path(state, label);
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", path));
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
    g_dbus_connection_emit_signal(state->local_bus, NULL, path,
                                  "org.freedesktop.portal.Request", "Response",
                                  g_variant_new("(u@a{sv})", 2,
                                                g_variant_builder_end(&results)),
                                  NULL);
    g_free(path);
}

static void forward_desktop_method(BridgeState *state, const char *interface_name,
                                   const char *method_name, GVariant *parameters,
                                   GDBusMethodInvocation *invocation)
{
    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop", interface_name, method_name,
                           parameters, NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           on_forward_call, g_object_ref(invocation));
}

static void return_settings_readall(GDBusMethodInvocation *invocation)
{
    GVariantBuilder values;
    g_variant_builder_init(&values, G_VARIANT_TYPE("a{sa{sv}}"));
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{sa{sv}})", &values));
}

static void return_proxy_direct(GDBusMethodInvocation *invocation)
{
    const char *direct[] = { "direct://", NULL };
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(^as)", direct));
}

static void handle_desktop_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)object_path;

    BridgeState *state = user_data;
    if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0 &&
        g_strcmp0(method_name, "OpenFile") == 0) {
        handle_filechooser_open(state, sender, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s is not implemented by this V1 bridge",
                                              method_name);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0 &&
               g_strcmp0(method_name, "ReadAll") == 0) {
        forward_desktop_method(state, interface_name, method_name, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0 &&
               g_strcmp0(method_name, "Read") == 0) {
        forward_desktop_method(state, interface_name, method_name, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ProxyResolver") == 0 &&
               g_strcmp0(method_name, "Lookup") == 0) {
        return_proxy_direct(invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
               (g_strcmp0(method_name, "CreateMonitor") == 0 ||
                g_strcmp0(method_name, "Inhibit") == 0)) {
        return_immediate_empty_request(state, invocation, method_name);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
               g_strcmp0(method_name, "QueryEndResponse") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s.%s is not implemented by this V1 bridge",
                                              interface_name, method_name);
    }
}

static void handle_add_full(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *parameters = g_dbus_method_invocation_get_parameters(invocation);
    GVariant *handles = g_variant_get_child_value(parameters, 0);
    GVariant *permissions = g_variant_get_child_value(parameters, 3);
    const char *app_id = NULL;
    g_variant_get_child(parameters, 2, "&s", &app_id);

    GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
    GUnixFDList *fd_list = g_dbus_message_get_unix_fd_list(message);
    if (fd_list == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                                              "AddFull did not include an fd list");
        g_variant_unref(handles);
        g_variant_unref(permissions);
        return;
    }

    GVariantBuilder ids;
    g_variant_builder_init(&ids, G_VARIANT_TYPE("as"));
    for (gsize i = 0; i < g_variant_n_children(handles); i++) {
        gint32 handle = -1;
        g_variant_get_child(handles, i, "h", &handle);

        GError *error = NULL;
        int fd = g_unix_fd_list_get(fd_list, handle, &error);
        if (fd < 0) {
            g_dbus_method_invocation_take_error(invocation, error);
            g_variant_unref(handles);
            g_variant_unref(permissions);
            return;
        }

        DocumentGrant *grant = NULL;
        if (!create_document_grant_from_fd(state, fd, app_id, permissions, &grant, &error)) {
            close(fd);
            g_dbus_method_invocation_take_error(invocation, error);
            g_variant_unref(handles);
            g_variant_unref(permissions);
            return;
        }
        close(fd);
        g_ptr_array_add(state->grants, grant);
        g_variant_builder_add(&ids, "s", grant->doc_id);
    }

    GVariantBuilder extra;
    g_variant_builder_init(&extra, G_VARIANT_TYPE("a{sv}"));
    add_mountpoint_extra(state, &extra);
    g_dbus_method_invocation_return_value(invocation,
                                          g_variant_new("(as@a{sv})", &ids,
                                                        g_variant_builder_end(&extra)));
    g_variant_unref(handles);
    g_variant_unref(permissions);
}

static void handle_delete(BridgeState *state, GDBusMethodInvocation *invocation)
{
    const char *doc_id = NULL;
    g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)", &doc_id);
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        if (g_strcmp0(grant->doc_id, doc_id) == 0) {
            cleanup_grant(grant);
            g_ptr_array_remove_index(state->grants, i);
            g_dbus_method_invocation_return_value(invocation, NULL);
            return;
        }
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                          "No such document: %s", doc_id);
}

static void return_lookup(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *path_variant =
        g_variant_get_child_value(g_dbus_method_invocation_get_parameters(invocation), 0);
    gsize size = 0;
    const gchar *path = g_variant_get_fixed_array(path_variant, &size, sizeof(guchar));
    const char *doc_id = "";
    if (path != NULL) {
        for (guint i = 0; i < state->grants->len; i++) {
            DocumentGrant *grant = g_ptr_array_index(state->grants, i);
            if (g_strcmp0(grant->host_path, path) == 0) {
                doc_id = grant->doc_id;
                break;
            }
        }
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(s)", doc_id));
    g_variant_unref(path_variant);
}

static void return_info(BridgeState *state, GDBusMethodInvocation *invocation)
{
    const char *doc_id = NULL;
    g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)", &doc_id);
    DocumentGrant *grant = find_grant(state, doc_id);
    if (grant == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                              "No such document: %s", doc_id);
        return;
    }

    GVariantBuilder apps;
    g_variant_builder_init(&apps, G_VARIANT_TYPE("a{sas}"));
    GVariantBuilder permissions;
    g_variant_builder_init(&permissions, G_VARIANT_TYPE("as"));
    for (char **p = grant->permissions; p != NULL && *p != NULL; p++) {
        g_variant_builder_add(&permissions, "s", *p);
    }
    g_variant_builder_add(&apps, "{s@as}", grant->app_id, g_variant_builder_end(&permissions));
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@aya{sas})", path_bytes_variant(grant->host_path), &apps));
}

static void return_list(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariantBuilder docs;
    g_variant_builder_init(&docs, G_VARIANT_TYPE("a{say}"));
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        g_variant_builder_add(&docs, "{s@ay}", grant->doc_id, path_bytes_variant(grant->host_path));
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{say})", &docs));
}

static void return_host_paths(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *doc_ids =
        g_variant_get_child_value(g_dbus_method_invocation_get_parameters(invocation), 0);
    GVariantBuilder paths;
    g_variant_builder_init(&paths, G_VARIANT_TYPE("a{say}"));

    GVariantIter iter;
    const char *doc_id = NULL;
    g_variant_iter_init(&iter, doc_ids);
    while (g_variant_iter_next(&iter, "&s", &doc_id)) {
        DocumentGrant *grant = find_grant(state, doc_id);
        if (grant != NULL) {
            g_variant_builder_add(&paths, "{s@ay}", grant->doc_id,
                                  path_bytes_variant(grant->host_path));
        }
    }

    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{say})", &paths));
    g_variant_unref(doc_ids);
}

static void handle_documents_method(GDBusConnection *connection, const gchar *sender,
                                    const gchar *object_path, const gchar *interface_name,
                                    const gchar *method_name, GVariant *parameters,
                                    GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)parameters;

    BridgeState *state = user_data;
    if (g_strcmp0(method_name, "GetMountPoint") == 0) {
        g_dbus_method_invocation_return_value(invocation,
                                              g_variant_new("(@ay)",
                                                            path_bytes_variant(state->mountpoint)));
    } else if (g_strcmp0(method_name, "AddFull") == 0) {
        handle_add_full(state, invocation);
    } else if (g_strcmp0(method_name, "GrantPermissions") == 0 ||
               g_strcmp0(method_name, "RevokePermissions") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else if (g_strcmp0(method_name, "Delete") == 0) {
        handle_delete(state, invocation);
    } else if (g_strcmp0(method_name, "Lookup") == 0) {
        return_lookup(state, invocation);
    } else if (g_strcmp0(method_name, "Info") == 0) {
        return_info(state, invocation);
    } else if (g_strcmp0(method_name, "List") == 0) {
        return_list(state, invocation);
    } else if (g_strcmp0(method_name, "GetHostPaths") == 0) {
        return_host_paths(state, invocation);
    } else {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s is not implemented by this V1 bridge",
                                              method_name);
    }
}

static GVariant *handle_get_property(GDBusConnection *connection, const gchar *sender,
                                     const gchar *object_path, const gchar *interface_name,
                                     const gchar *property_name, GError **error,
                                     gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)user_data;

    if (g_strcmp0(property_name, "version") == 0) {
        if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0) {
            return g_variant_new_uint32(4);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Documents") == 0) {
            return g_variant_new_uint32(5);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0) {
            return g_variant_new_uint32(2);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.ProxyResolver") == 0) {
            return g_variant_new_uint32(1);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0) {
            return g_variant_new_uint32(3);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.FileTransfer") == 0) {
            return g_variant_new_uint32(1);
        }
    }
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s.%s",
                interface_name, property_name);
    return NULL;
}

static const GDBusInterfaceVTable DESKTOP_VTABLE = {
    .method_call = handle_desktop_method,
    .get_property = handle_get_property,
};

static const GDBusInterfaceVTable DOCUMENTS_VTABLE = {
    .method_call = handle_documents_method,
    .get_property = handle_get_property,
};

static bool register_node_interfaces(GDBusConnection *connection, const char *path,
                                     GDBusNodeInfo *node, const GDBusInterfaceVTable *vtable,
                                     BridgeState *state, GError **error)
{
    for (guint i = 0; node->interfaces[i] != NULL; i++) {
        guint id =
            g_dbus_connection_register_object(connection, path, node->interfaces[i], vtable,
                                              state, NULL, error);
        if (id == 0) {
            return false;
        }
    }
    return true;
}

static void on_bus_acquired(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)name;
    BridgeState *state = user_data;
    if (state->local_bus == NULL) {
        state->local_bus = g_object_ref(connection);
    }
    if (state->local_objects_registered) {
        return;
    }

    GError *error = NULL;
    if (!register_node_interfaces(connection, "/org/freedesktop/portal/desktop",
                                  state->desktop_node, &DESKTOP_VTABLE, state, &error)) {
        log_line("register desktop portal failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    if (!register_node_interfaces(connection, "/org/freedesktop/portal/documents",
                                  state->documents_node, &DOCUMENTS_VTABLE, state, &error)) {
        log_line("register documents portal failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    state->local_objects_registered = true;
}

static void on_name_acquired(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)connection;
    (void)user_data;
    log_line("acquired %s", name);
}

static void on_name_lost(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)connection;
    BridgeState *state = user_data;
    log_line("lost %s", name);
    g_main_loop_quit(state->loop);
}

static const char *arg_value(int argc, char **argv, const char *name)
{
    for (int i = 1; i + 1 < argc; i++) {
        if (strcmp(argv[i], name) == 0) {
            return argv[i + 1];
        }
    }
    return NULL;
}

static GDBusConnection *connect_to_bus_address(const char *address, GError **error)
{
    return g_dbus_connection_new_for_address_sync(
        address,
        G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
            G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
        NULL, NULL, error);
}

int main(int argc, char **argv)
{
    const char *app_id = arg_value(argc, argv, "--app-id");
    const char *doc_dir = arg_value(argc, argv, "--doc-dir");
    const char *sandbox_doc_dir = arg_value(argc, argv, "--sandbox-doc-dir");
    const char *mountpoint = arg_value(argc, argv, "--mountpoint");
    const char *host_bus_address = getenv("HOST_DBUS_SESSION_BUS_ADDRESS");
    if (app_id == NULL || doc_dir == NULL || sandbox_doc_dir == NULL || mountpoint == NULL ||
        host_bus_address == NULL || *host_bus_address == '\0') {
        fprintf(stderr,
                "usage: %s --app-id APP_ID --doc-dir HOST_DOC_DIR --sandbox-doc-dir CHROOT_DOC_DIR --mountpoint SANDBOX_MOUNTPOINT\n",
                argv[0]);
        fprintf(stderr, "HOST_DBUS_SESSION_BUS_ADDRESS must point at the host session bus\n");
        return 64;
    }

    if (g_mkdir_with_parents(doc_dir, 0700) != 0) {
        fprintf(stderr, "create %s failed: %s\n", doc_dir, g_strerror(errno));
        return 1;
    }

    GError *error = NULL;
    BridgeState state = {
        .app_id = g_strdup(app_id),
        .doc_dir = g_strdup(doc_dir),
        .sandbox_doc_dir = g_strdup(sandbox_doc_dir),
        .mountpoint = g_strdup(mountpoint),
        .grants = g_ptr_array_new_with_free_func((GDestroyNotify)free_grant),
        .requests = g_ptr_array_new_with_free_func((GDestroyNotify)free_request),
        .counter = 0,
        .request_counter = 0,
        .loop = g_main_loop_new(NULL, FALSE),
        .host_bus = connect_to_bus_address(host_bus_address, &error),
        .local_bus = NULL,
        .desktop_node = g_dbus_node_info_new_for_xml(DESKTOP_XML, &error),
        .documents_node = NULL,
        .request_node = NULL,
    };
    if (state.host_bus == NULL || state.desktop_node == NULL) {
        fprintf(stderr, "portal bridge setup failed: %s\n", error->message);
        g_error_free(error);
        return 1;
    }
    state.documents_node = g_dbus_node_info_new_for_xml(DOCUMENTS_XML, &error);
    state.request_node = g_dbus_node_info_new_for_xml(REQUEST_XML, &error);
    if (state.documents_node == NULL || state.request_node == NULL) {
        fprintf(stderr, "portal bridge introspection failed: %s\n", error->message);
        g_error_free(error);
        return 1;
    }

    g_unix_signal_add(SIGINT, handle_signal, &state);
    g_unix_signal_add(SIGTERM, handle_signal, &state);

    guint desktop_owner_id =
        g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.portal.Desktop",
                       G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT |
                           G_BUS_NAME_OWNER_FLAGS_REPLACE,
                       on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
    guint documents_owner_id =
        g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.portal.Documents",
                       G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT |
                           G_BUS_NAME_OWNER_FLAGS_REPLACE,
                       on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
    log_line("serving private portal for %s at %s", state.app_id, state.doc_dir);
    g_main_loop_run(state.loop);

    cleanup_all(&state);
    g_bus_unown_name(documents_owner_id);
    g_bus_unown_name(desktop_owner_id);
    if (state.local_bus != NULL) {
        g_object_unref(state.local_bus);
    }
    if (state.host_bus != NULL) {
        g_object_unref(state.host_bus);
    }
    g_dbus_node_info_unref(state.desktop_node);
    g_dbus_node_info_unref(state.documents_node);
    g_dbus_node_info_unref(state.request_node);
    g_main_loop_unref(state.loop);
    g_ptr_array_free(state.requests, TRUE);
    g_ptr_array_free(state.grants, TRUE);
    g_free(state.app_id);
    g_free(state.doc_dir);
    g_free(state.sandbox_doc_dir);
    g_free(state.mountpoint);
    return 0;
}
