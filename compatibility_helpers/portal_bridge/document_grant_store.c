#include "document_grant_store.h"
#include "document_grant_persistence.h"
#include "document_id.h"
#include "document_mounts.h"
#include "portal_bridge_process.h"
GVariant *path_bytes_variant(const char *path) {
  gsize len = strlen(path) + 1;
  return g_variant_new_fixed_array(G_VARIANT_TYPE_BYTE, path, len,
                                   sizeof(guchar));
}

char **read_permissions(void) {
  char **permissions = g_new0(char *, 2);
  permissions[0] = g_strdup("read");
  return permissions;
}

char **read_write_permissions(void) {
  char **permissions = g_new0(char *, 3);
  permissions[0] = g_strdup("read");
  permissions[1] = g_strdup("write");
  return permissions;
}

void free_grant(DocumentGrant *grant) {
  if (grant == NULL) {
    return;
  }
  g_free(grant->doc_id);
  g_free(grant->host_path);
  g_free(grant->placeholder_path);
  if (grant->target_paths != NULL) {
    g_ptr_array_free(grant->target_paths, TRUE);
  }
  g_free(grant->app_id);
  g_strfreev(grant->permissions);
  g_free(grant);
}

bool fd_host_path(int fd, char *path, size_t path_len, GError **error) {
  struct kinfo_file info;
  memset(&info, 0, sizeof(info));
  info.kf_structsize = sizeof(info);
  if (fcntl(fd, F_KINFO, &info) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "F_KINFO failed: %s", g_strerror(errno));
    return false;
  }
  if (info.kf_type != KF_TYPE_VNODE || info.kf_path[0] == '\0') {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "selected FD is not a path-backed file or directory");
    return false;
  }
  g_strlcpy(path, info.kf_path, path_len);
  return true;
}

char **permissions_from_variant(GVariant *permissions) {
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

void merge_document_permissions(DocumentGrant *grant, char **permissions) {
  GPtrArray *merged = g_ptr_array_new_with_free_func(g_free);
  for (char **p = grant->permissions; p != NULL && *p != NULL; p++) {
    g_ptr_array_add(merged, g_strdup(*p));
  }
  for (char **p = permissions; p != NULL && *p != NULL; p++) {
    bool found = false;
    for (guint i = 0; i < merged->len; i++) {
      if (g_strcmp0(g_ptr_array_index(merged, i), *p) == 0) {
        found = true;
        break;
      }
    }
    if (!found) {
      g_ptr_array_add(merged, g_strdup(*p));
    }
  }
  g_ptr_array_add(merged, NULL);
  g_strfreev(grant->permissions);
  grant->permissions = (char **)g_ptr_array_free(merged, FALSE);
}

bool prepare_document_grant(BridgeState *state, const char *doc_id,
                            const char *host_path, const char *app_id,
                            char **permissions, bool expected_directory,
                            bool persistent, DocumentGrant **out,
                            GError **error) {
  struct stat st;
  if (g_stat(host_path, &st) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "stat %s failed: %s", host_path, g_strerror(errno));
    return false;
  }
  bool is_directory = S_ISDIR(st.st_mode);
  if ((!is_directory && !S_ISREG(st.st_mode)) ||
      is_directory != expected_directory) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "selected path has an unexpected type: %s", host_path);
    return false;
  }

  char *base = g_path_get_basename(host_path);
  char *source_doc_dir =
      g_build_filename(state->documents.doc_dir, doc_id, NULL);
  if (g_mkdir_with_parents(source_doc_dir, 0700) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "create %s failed: %s", source_doc_dir, g_strerror(errno));
    g_free(base);
    g_free(source_doc_dir);
    return false;
  }

  char *placeholder = g_build_filename(source_doc_dir, base, NULL);
  int placeholder_fd = -1;
  int placeholder_result = 0;
  if (is_directory) {
    placeholder_result = g_mkdir(placeholder, 0700);
  } else {
    placeholder_fd = g_open(placeholder, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    placeholder_result = placeholder_fd < 0 ? -1 : 0;
  }
  if (placeholder_result != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "create %s failed: %s", placeholder, g_strerror(errno));
    g_free(base);
    g_free(source_doc_dir);
    g_free(placeholder);
    return false;
  }
  if (placeholder_fd >= 0) {
    close(placeholder_fd);
  }

  DocumentGrant *grant = g_new0(DocumentGrant, 1);
  grant->doc_id = g_strdup(doc_id);
  grant->host_path = g_strdup(host_path);
  grant->placeholder_path = placeholder;
  grant->target_paths = g_ptr_array_new_with_free_func(g_free);
  grant->app_id =
      g_strdup(app_id != NULL && *app_id != '\0' ? app_id : state->app_id);
  grant->permissions =
      permissions != NULL ? g_strdupv(permissions) : read_permissions();
  grant->is_directory = is_directory;
  grant->persistent = persistent;

  for (guint i = 0; i < state->documents.sandbox_doc_dirs->len; i++) {
    if (!mount_grant_in_sandbox(
            grant, g_ptr_array_index(state->documents.sandbox_doc_dirs, i),
            error)) {
      cleanup_grant(grant);
      free_grant(grant);
      g_free(base);
      g_free(source_doc_dir);
      return false;
    }
  }
  *out = grant;

  diagnostic_line("%s -> %u sandbox(s) as %s/%s", grant->host_path,
                  grant->target_paths->len, grant->doc_id, base);
  g_free(base);
  g_free(source_doc_dir);
  return true;
}

bool create_document_grant_from_path(BridgeState *state, const char *host_path,
                                     const char *app_id, char **permissions,
                                     bool expected_directory, bool persistent,
                                     bool reuse_existing, DocumentGrant **out,
                                     GError **error) {
  char *resolved_path = realpath(host_path, NULL);
  if (resolved_path == NULL) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "resolve %s failed: %s", host_path, g_strerror(errno));
    return false;
  }
  if (reuse_existing) {
    DocumentGrant *existing = find_reusable_grant(
        state, resolved_path, expected_directory, persistent);
    if (existing != NULL) {
      merge_document_permissions(existing, permissions);
      *out = existing;
      free(resolved_path);
      return true;
    }
  }
  char *doc_id = generate_document_id(state);
  bool created = prepare_document_grant(
      state, doc_id, resolved_path, app_id, permissions, expected_directory,
      persistent, out, error);
  g_free(doc_id);
  free(resolved_path);
  return created;
}

bool restore_document_grant(BridgeState *state, const char *doc_id,
                            const char *host_path, const char *app_id,
                            char **permissions, bool expected_directory,
                            DocumentGrant **out, GError **error) {
  if (!document_id_is_valid(doc_id)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "invalid persistent document id");
    return false;
  }
  return prepare_document_grant(state, doc_id, host_path, app_id, permissions,
                                expected_directory, true, out, error);
}

bool register_document_grant(BridgeState *state, DocumentGrant *grant,
                             GError **error) {
  bool registered = false;
  for (guint i = 0; i < state->documents.grants->len; i++) {
    if (g_ptr_array_index(state->documents.grants, i) == grant) {
      registered = true;
      break;
    }
  }
  if (!registered) {
    g_ptr_array_add(state->documents.grants, grant);
  }
  if (!grant->persistent || save_persistent_document_grants(state, error)) {
    return true;
  }
  if (!registered) {
    cleanup_grant(grant);
    g_ptr_array_remove(state->documents.grants, grant);
  }
  return false;
}

bool create_document_grant_from_fd(BridgeState *state, int fd,
                                   const char *app_id, GVariant *permissions,
                                   bool expected_directory, bool persistent,
                                   bool reuse_existing, DocumentGrant **out,
                                   GError **error) {
  char host_path[PATH_MAX];
  if (!fd_host_path(fd, host_path, sizeof(host_path), error)) {
    return false;
  }
  char **permission_list = permissions_from_variant(permissions);
  bool ok = create_document_grant_from_path(state, host_path, app_id,
                                            permission_list,
                                            expected_directory, persistent,
                                            reuse_existing, out, error);
  g_strfreev(permission_list);
  return ok;
}

DocumentGrant *find_grant(BridgeState *state, const char *doc_id) {
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    if (g_strcmp0(grant->doc_id, doc_id) == 0) {
      return grant;
    }
  }
  return NULL;
}

DocumentGrant *find_reusable_grant(BridgeState *state, const char *host_path,
                                   bool is_directory, bool persistent) {
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    if (g_strcmp0(grant->host_path, host_path) == 0 &&
        grant->is_directory == is_directory &&
        grant->persistent == persistent) {
      return grant;
    }
  }
  return NULL;
}

void add_mountpoint_extra(BridgeState *state, GVariantBuilder *extra) {
  g_variant_builder_add(extra, "{sv}", "mountpoint",
                        path_bytes_variant(state->documents.mountpoint));
}

char *sandbox_uri_for_grant(BridgeState *state, DocumentGrant *grant) {
  char *base = g_path_get_basename(grant->host_path);
  char *sandbox_path =
      g_build_filename(state->documents.mountpoint, grant->doc_id, base, NULL);
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

char *host_path_for_document_path(BridgeState *state, const char *path) {
  char *prefix = g_strconcat(state->documents.mountpoint, G_DIR_SEPARATOR_S,
                             NULL);
  if (!g_str_has_prefix(path, prefix)) {
    g_free(prefix);
    return NULL;
  }
  const char *relative = path + strlen(prefix);
  const char *separator = strchr(relative, G_DIR_SEPARATOR);
  if (separator == NULL || separator == relative) {
    g_free(prefix);
    return NULL;
  }
  char *doc_id = g_strndup(relative, (gsize)(separator - relative));
  DocumentGrant *grant = find_grant(state, doc_id);
  g_free(doc_id);
  if (grant == NULL) {
    g_free(prefix);
    return NULL;
  }

  const char *visible_path = separator + 1;
  char *base = g_path_get_basename(grant->host_path);
  gsize base_length = strlen(base);
  bool matches_base = strncmp(visible_path, base, base_length) == 0 &&
                      (visible_path[base_length] == '\0' ||
                       visible_path[base_length] == G_DIR_SEPARATOR);
  char *host_path = NULL;
  if (matches_base) {
    const char *suffix = visible_path + base_length;
    bool safe_suffix = grant->is_directory || *suffix == '\0';
    safe_suffix = safe_suffix && strstr(suffix, "/../") == NULL &&
                  !g_str_has_suffix(suffix, "/..") &&
                  !g_str_has_prefix(suffix, "../");
    if (safe_suffix) {
      host_path = g_strconcat(grant->host_path, suffix, NULL);
    }
  }
  g_free(base);
  g_free(prefix);
  return host_path;
}
