#include "document_grant_store.h"
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
  if (info.kf_type != KF_TYPE_VNODE ||
      info.kf_un.kf_file.kf_file_type != KF_VTYPE_VREG ||
      info.kf_path[0] == '\0') {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "selected FD is not a regular path-backed file");
    return false;
  }
  g_strlcpy(path, info.kf_path, path_len);
  return true;
}

char *safe_doc_id(BridgeState *state) {
  GString *id = g_string_new("freebsd_flatpak_poc_");
  for (const char *p = state->app_id; *p != '\0'; p++) {
    if (g_ascii_isalnum(*p)) {
      g_string_append_c(id, *p);
    } else {
      g_string_append_c(id, '_');
    }
  }
  g_string_append_printf(id, "_%" G_GUINT64_FORMAT, ++state->documents.counter);
  return g_string_free(id, FALSE);
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

bool create_document_grant_from_path(BridgeState *state, const char *host_path,
                                     const char *app_id, char **permissions,
                                     DocumentGrant **out, GError **error) {
  struct stat st;
  if (g_stat(host_path, &st) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "stat %s failed: %s", host_path, g_strerror(errno));
    return false;
  }
  if (!S_ISREG(st.st_mode)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_REGULAR_FILE,
                "FileChooser V1 only grants regular files: %s", host_path);
    return false;
  }

  char *base = g_path_get_basename(host_path);
  char *doc_id = safe_doc_id(state);
  char *source_doc_dir =
      g_build_filename(state->documents.doc_dir, doc_id, NULL);
  if (g_mkdir_with_parents(source_doc_dir, 0700) != 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "create %s failed: %s", source_doc_dir, g_strerror(errno));
    g_free(base);
    g_free(doc_id);
    g_free(source_doc_dir);
    return false;
  }

  char *placeholder = g_build_filename(source_doc_dir, base, NULL);
  int placeholder_fd = g_open(placeholder, O_CREAT | O_TRUNC | O_WRONLY, 0600);
  if (placeholder_fd < 0) {
    g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                "create %s failed: %s", placeholder, g_strerror(errno));
    g_free(base);
    g_free(doc_id);
    g_free(source_doc_dir);
    g_free(placeholder);
    return false;
  }
  close(placeholder_fd);

  DocumentGrant *grant = g_new0(DocumentGrant, 1);
  grant->doc_id = doc_id;
  grant->host_path = g_strdup(host_path);
  grant->placeholder_path = placeholder;
  grant->target_paths = g_ptr_array_new_with_free_func(g_free);
  grant->app_id =
      g_strdup(app_id != NULL && *app_id != '\0' ? app_id : state->app_id);
  grant->permissions =
      permissions != NULL ? g_strdupv(permissions) : read_permissions();

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

  log_line("%s -> %u sandbox(s) as %s/%s", grant->host_path,
           grant->target_paths->len, grant->doc_id, base);
  g_free(base);
  g_free(source_doc_dir);
  return true;
}

bool create_document_grant_from_fd(BridgeState *state, int fd,
                                   const char *app_id, GVariant *permissions,
                                   DocumentGrant **out, GError **error) {
  char host_path[PATH_MAX];
  if (!fd_host_path(fd, host_path, sizeof(host_path), error)) {
    return false;
  }
  char **permission_list = permissions_from_variant(permissions);
  bool ok = create_document_grant_from_path(state, host_path, app_id,
                                            permission_list, out, error);
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
