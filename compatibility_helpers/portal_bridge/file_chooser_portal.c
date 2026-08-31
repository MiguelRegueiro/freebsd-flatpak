#include <sys/mount.h>

#include "file_chooser_portal.h"
#include "document_grant_store.h"
#include "portal_bridge_process.h"
#include "portal_request.h"

static bool path_is_at_or_below(const char *path, const char *root) {
  gsize root_length = strlen(root);
  return g_str_has_prefix(path, root) &&
         (root_length == 1 || path[root_length] == 0 ||
          path[root_length] == G_DIR_SEPARATOR);
}

static bool sandbox_mount_maps_to_host(int sandbox_fd,
                                       const char *sandbox_path,
                                       const struct stat *host_stat) {
  struct statfs sandbox_fs;
  if (fstatfs(sandbox_fd, &sandbox_fs) != 0 ||
      !path_is_at_or_below(sandbox_path, sandbox_fs.f_mntonname)) {
    return false;
  }

  const char *suffix = g_str_equal(sandbox_fs.f_mntonname, G_DIR_SEPARATOR_S)
                           ? sandbox_path
                           : sandbox_path + strlen(sandbox_fs.f_mntonname);
  char *mapped_path = g_strconcat(sandbox_fs.f_mntfromname, suffix, NULL);
  int mapped_fd = open(mapped_path, O_RDONLY | O_CLOEXEC);
  g_free(mapped_path);
  struct stat mapped_stat;
  bool matches = mapped_fd >= 0 && fstat(mapped_fd, &mapped_stat) == 0 &&
                 mapped_stat.st_dev == host_stat->st_dev &&
                 mapped_stat.st_ino == host_stat->st_ino;
  if (mapped_fd >= 0) {
    close(mapped_fd);
  }
  return matches;
}

static bool sandbox_path_matches_host(const char *sandbox_doc_dir,
                                      const char *mountpoint,
                                      const char *host_path) {
  gsize doc_dir_length = strlen(sandbox_doc_dir);
  gsize mountpoint_length = strlen(mountpoint);
  if (doc_dir_length <= mountpoint_length ||
      !g_str_has_suffix(sandbox_doc_dir, mountpoint)) {
    return false;
  }

  char *sandbox_root = g_strndup(sandbox_doc_dir,
                                 doc_dir_length - mountpoint_length);
  int root_fd = open(sandbox_root, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  char *sandbox_path = g_strconcat(sandbox_root, host_path, NULL);
  g_free(sandbox_root);
  if (root_fd < 0) {
    g_free(sandbox_path);
    return false;
  }

  const char *relative_path = host_path;
  while (*relative_path == G_DIR_SEPARATOR) {
    relative_path++;
  }
  int sandbox_fd = openat(root_fd, relative_path,
                          O_RDONLY | O_CLOEXEC | O_RESOLVE_BENEATH);
  int host_fd = open(host_path, O_RDONLY | O_CLOEXEC);
  struct stat sandbox_stat;
  struct stat host_stat;
  /* nullfs preserves the source inode but presents a different st_dev. */
  bool stats_valid = sandbox_fd >= 0 && host_fd >= 0 &&
                     fstat(sandbox_fd, &sandbox_stat) == 0 &&
                     fstat(host_fd, &host_stat) == 0;
  bool matches = stats_valid && sandbox_stat.st_dev == host_stat.st_dev &&
                 sandbox_stat.st_ino == host_stat.st_ino;
  if (!matches && stats_valid) {
    matches = sandbox_mount_maps_to_host(sandbox_fd, sandbox_path, &host_stat);
  }
  if (sandbox_fd >= 0) {
    close(sandbox_fd);
  }
  if (host_fd >= 0) {
    close(host_fd);
  }
  close(root_fd);
  g_free(sandbox_path);
  return matches;
}

static bool app_has_direct_access(BridgeState *state, const char *host_path) {
  if (state->documents.sandbox_doc_dirs->len == 0) {
    return false;
  }
  for (guint i = 0; i < state->documents.sandbox_doc_dirs->len; i++) {
    const char *sandbox_doc_dir =
        g_ptr_array_index(state->documents.sandbox_doc_dirs, i);
    if (!sandbox_path_matches_host(sandbox_doc_dir,
                                   state->documents.mountpoint, host_path)) {
      return false;
    }
  }
  return true;
}
char *rewrite_file_uri(BridgeState *state, const char *uri, bool directory) {
  GError *error = NULL;
  char *host_path = g_filename_from_uri(uri, NULL, &error);
  if (host_path == NULL) {
    log_line("could not decode FileChooser URI %s: %s", uri, error->message);
    g_error_free(error);
    return NULL;
  }

  if (app_has_direct_access(state, host_path)) {
    diagnostic_line("kept directly accessible FileChooser URI %s", uri);
    g_free(host_path);
    return g_strdup(uri);
  }

  char **permissions = directory ? read_write_permissions()
                                 : read_permissions();
  DocumentGrant *grant = NULL;
  if (!create_document_grant_from_path(state, host_path, state->app_id,
                                       permissions, directory, true, true,
                                       &grant, &error)) {
    log_line("could not grant %s: %s", host_path, error->message);
    g_error_free(error);
    g_strfreev(permissions);
    g_free(host_path);
    return NULL;
  }
  if (!register_document_grant(state, grant, &error)) {
    log_line("could not persist grant for %s: %s", host_path, error->message);
    g_error_free(error);
    g_strfreev(permissions);
    g_free(host_path);
    return NULL;
  }
  g_strfreev(permissions);
  g_free(host_path);

  char *rewritten = sandbox_uri_for_grant(state, grant);
  if (rewritten != NULL) {
    diagnostic_line("rewrote FileChooser URI to %s", rewritten);
  }
  return rewritten;
}

GVariant *rewrite_uri_array(BridgeState *state, GVariant *uris,
                            bool directory) {
  GVariantBuilder rewritten;
  g_variant_builder_init(&rewritten, G_VARIANT_TYPE("as"));

  GVariantIter iter;
  const char *uri = NULL;
  g_variant_iter_init(&iter, uris);
  while (g_variant_iter_next(&iter, "&s", &uri)) {
    char *mapped = NULL;
    if (g_str_has_prefix(uri, "file://")) {
      mapped = rewrite_file_uri(state, uri, directory);
    }
    if (mapped != NULL) {
      g_variant_builder_add(&rewritten, "s", mapped);
    } else {
      log_line("discarded ungrantable FileChooser URI %s", uri);
    }
    g_free(mapped);
  }

  return g_variant_builder_end(&rewritten);
}

GVariant *rewrite_filechooser_results(BridgeState *state, guint32 response,
                                      GVariant *results, bool directory) {
  GVariantBuilder out;
  g_variant_builder_init(&out, G_VARIANT_TYPE("a{sv}"));

  GVariantIter iter;
  const char *key = NULL;
  GVariant *value = NULL;
  g_variant_iter_init(&iter, results);
  while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
    if (response == 0 && g_strcmp0(key, "uris") == 0 &&
        g_variant_is_of_type(value, G_VARIANT_TYPE("as"))) {
      GVariant *rewritten = rewrite_uri_array(state, value, directory);
      g_variant_builder_add(&out, "{sv}", key, rewritten);
    } else {
      g_variant_builder_add(&out, "{sv}", key, value);
    }
    g_variant_unref(value);
  }

  return g_variant_builder_end(&out);
}

GVariant *rewrite_filechooser_parameters(BridgeState *state,
                                         GVariant *parameters) {
  const char *parent_window = NULL;
  const char *title = NULL;
  GVariant *options = NULL;
  g_variant_get(parameters, "(&s&s@a{sv})", &parent_window, &title, &options);
  GVariantBuilder rewritten;
  g_variant_builder_init(&rewritten, G_VARIANT_TYPE_VARDICT);
  GVariantIter iter;
  const char *key = NULL;
  GVariant *value = NULL;
  g_variant_iter_init(&iter, options);
  while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
    if (g_strcmp0(key, "current_folder") == 0 &&
        g_variant_is_of_type(value, G_VARIANT_TYPE_BYTESTRING)) {
      const char *path = g_variant_get_bytestring(value);
      char *host_path = host_path_for_document_path(state, path);
      if (host_path != NULL) {
        g_variant_builder_add(&rewritten, "{sv}", key,
                              g_variant_new_bytestring(host_path));
        g_free(host_path);
        g_variant_unref(value);
        continue;
      }
    }
    g_variant_builder_add(&rewritten, "{sv}", key, value);
    g_variant_unref(value);
  }
  g_variant_unref(options);
  return g_variant_new("(ss@a{sv})", parent_window, title,
                       g_variant_builder_end(&rewritten));
}

void on_host_response(GDBusConnection *connection, const gchar *sender_name,
                      const gchar *object_path, const gchar *interface_name,
                      const gchar *signal_name, GVariant *parameters,
                      gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;

  RequestRecord *request = user_data;
  guint32 response = 2;
  GVariant *results = NULL;
  g_variant_get(parameters, "(u@a{sv})", &response, &results);
  GVariant *rewritten =
      rewrite_filechooser_results(request->state, response, results,
                                  request->filechooser_directory);
  g_variant_unref(results);
  emit_request_response(request, response, rewritten);

  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
    request->host_signal_id = 0;
  }
}

void on_host_filechooser_call(GObject *source_object, GAsyncResult *result,
                              gpointer user_data) {
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
  g_free(request->host_path);
  request->host_path = g_strdup(host_handle);
  request->host_signal_id = g_dbus_connection_signal_subscribe(
      request->state->host_bus, "org.freedesktop.portal.Desktop",
      "org.freedesktop.portal.Request", "Response", host_handle, NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_host_response, request, NULL);
  if (request->close_requested) {
    g_dbus_connection_call(request->state->host_bus,
                           "org.freedesktop.portal.Desktop", host_handle,
                           "org.freedesktop.portal.Request", "Close", NULL,
                           NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
  }
  g_variant_unref(reply);
}
void handle_filechooser_open(BridgeState *state, const char *sender,
                             GVariant *parameters,
                             GDBusMethodInvocation *invocation) {
  GError *error = NULL;
  char *local_path = request_path_for_call(state, sender, parameters, 2);
  GDBusInterfaceInfo *request_iface = g_dbus_node_info_lookup_interface(
      state->request_node, "org.freedesktop.portal.Request");
  RequestRecord *request = g_new0(RequestRecord, 1);
  request->state = state;
  request->client_sender = g_strdup(sender);
  request->local_path = g_strdup(local_path);
  request->kind = REQUEST_FILECHOOSER;
  GVariant *options = g_variant_get_child_value(parameters, 2);
  gboolean directory = FALSE;
  g_variant_lookup(options, "directory", "b", &directory);
  request->filechooser_directory = directory;
  g_variant_unref(options);
  request->local_registration_id = g_dbus_connection_register_object(
      state->local_bus, local_path, request_iface, &REQUEST_VTABLE, state, NULL,
      &error);
  if (request->local_registration_id == 0) {
    g_dbus_method_invocation_take_error(invocation, error);
    g_free(request->client_sender);
    g_free(request->local_path);
    g_free(request);
    g_free(local_path);
    return;
  }
  g_ptr_array_add(state->request_store.requests, request);

  GVariant *host_parameters =
      rewrite_filechooser_parameters(state, parameters);
  g_dbus_connection_call(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "/org/freedesktop/portal/desktop", "org.freedesktop.portal.FileChooser",
      "OpenFile", host_parameters, G_VARIANT_TYPE("(o)"),
      G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_host_filechooser_call, request);

  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(o)", local_path));
  diagnostic_line("forwarded FileChooser.OpenFile as %s", local_path);
  g_free(local_path);
}
