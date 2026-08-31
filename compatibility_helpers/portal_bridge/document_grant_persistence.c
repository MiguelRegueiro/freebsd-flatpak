#include "document_grant_persistence.h"
#include "document_grant_store.h"
#include "portal_bridge_process.h"

bool save_persistent_document_grants(BridgeState *state, GError **error) {
  GKeyFile *key_file = g_key_file_new();
  guint persistent_count = 0;
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    if (!grant->persistent) {
      continue;
    }
    persistent_count++;
    g_key_file_set_string(key_file, grant->doc_id, "path", grant->host_path);
    g_key_file_set_string(key_file, grant->doc_id, "app-id", grant->app_id);
    g_key_file_set_boolean(key_file, grant->doc_id, "directory",
                           grant->is_directory);
    gsize permission_count = g_strv_length(grant->permissions);
    g_key_file_set_string_list(key_file, grant->doc_id, "permissions",
                               (const gchar *const *)grant->permissions,
                               permission_count);
  }

  if (persistent_count == 0) {
    g_key_file_unref(key_file);
    if (g_remove(state->documents.persistent_store) == 0 || errno == ENOENT) {
      return true;
    }
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "remove %s failed: %s", state->documents.persistent_store,
                g_strerror(errno));
    return false;
  }

  gsize length = 0;
  char *data = g_key_file_to_data(key_file, &length, error);
  g_key_file_unref(key_file);
  if (data == NULL) {
    return false;
  }
  bool saved = g_file_set_contents(state->documents.persistent_store, data,
                                   (gssize)length, error);
  g_free(data);
  if (saved && g_chmod(state->documents.persistent_store, 0600) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "chmod %s failed: %s", state->documents.persistent_store,
                g_strerror(errno));
    return false;
  }
  return saved;
}

bool load_persistent_document_grants(BridgeState *state, GError **error) {
  GKeyFile *key_file = g_key_file_new();
  GError *load_error = NULL;
  if (!g_key_file_load_from_file(key_file, state->documents.persistent_store,
                                 G_KEY_FILE_NONE, &load_error)) {
    if (g_error_matches(load_error, G_FILE_ERROR, G_FILE_ERROR_NOENT)) {
      g_error_free(load_error);
      g_key_file_unref(key_file);
      return true;
    }
    g_propagate_error(error, load_error);
    g_key_file_unref(key_file);
    return false;
  }

  bool discarded_missing = false;
  gsize group_count = 0;
  char **groups = g_key_file_get_groups(key_file, &group_count);
  for (gsize i = 0; i < group_count; i++) {
    GError *field_error = NULL;
    char *path =
        g_key_file_get_string(key_file, groups[i], "path", &field_error);
    char *app_id = field_error == NULL
                       ? g_key_file_get_string(key_file, groups[i], "app-id",
                                               &field_error)
                       : NULL;
    gboolean directory =
        field_error == NULL
            ? g_key_file_get_boolean(key_file, groups[i], "directory",
                                     &field_error)
            : FALSE;
    gsize permission_count = 0;
    char **permissions =
        field_error == NULL
            ? g_key_file_get_string_list(key_file, groups[i], "permissions",
                                         &permission_count, &field_error)
            : NULL;
    if (path == NULL || app_id == NULL || permissions == NULL) {
      g_free(path);
      g_free(app_id);
      g_strfreev(permissions);
      g_strfreev(groups);
      g_key_file_unref(key_file);
      g_propagate_error(error, field_error);
      return false;
    }

    DocumentGrant *grant = NULL;
    GError *restore_error = NULL;
    if (!restore_document_grant(state, groups[i], path, app_id, permissions,
                                directory, &grant, &restore_error)) {
      if (g_error_matches(restore_error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND)) {
        diagnostic_line("discarded missing persistent document grant %s: %s",
                        groups[i], path);
        g_clear_error(&restore_error);
        discarded_missing = true;
        g_free(path);
        g_free(app_id);
        g_strfreev(permissions);
        continue;
      }
      g_free(path);
      g_free(app_id);
      g_strfreev(permissions);
      g_strfreev(groups);
      g_key_file_unref(key_file);
      g_propagate_error(error, restore_error);
      return false;
    }
    g_ptr_array_add(state->documents.grants, grant);
    g_free(path);
    g_free(app_id);
    g_strfreev(permissions);
  }
  g_strfreev(groups);
  g_key_file_unref(key_file);
  return !discarded_missing || save_persistent_document_grants(state, error);
}
