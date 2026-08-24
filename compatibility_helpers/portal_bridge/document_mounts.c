#include "document_mounts.h"
#include "document_mount_backend.h"
#include "document_grant_store.h"
#include "portal_bridge_process.h"
void cleanup_grant(DocumentGrant *grant) {
  if (grant == NULL) {
    return;
  }
  if (grant->target_paths != NULL) {
    for (guint i = 0; i < grant->target_paths->len; i++) {
      unmount_path(g_ptr_array_index(grant->target_paths, i));
    }
    g_ptr_array_set_size(grant->target_paths, 0);
  }
  const char *placeholder = grant->placeholder_path;
  if (placeholder == NULL) {
    return;
  }
  char *document_dir = g_path_get_dirname(placeholder);
  int remove_result = grant->is_directory ? g_rmdir(placeholder)
                                          : g_remove(placeholder);
  if (remove_result != 0 && errno != ENOENT) {
    log_line("remove placeholder %s failed: %s", placeholder,
             g_strerror(errno));
  }
  if (g_rmdir(document_dir) != 0 && errno != ENOENT) {
    log_line("remove %s failed: %s", document_dir, g_strerror(errno));
  }
  g_free(document_dir);
}

bool sandbox_doc_dir_allowed(BridgeState *state, const char *path) {
  if (path == NULL || !g_path_is_absolute(path)) {
    return false;
  }
  char *root_prefix =
      g_strconcat(state->documents.sandbox_root, G_DIR_SEPARATOR_S, NULL);
  bool allowed = g_str_has_prefix(path, root_prefix) &&
                 strstr(path, "/../") == NULL && !g_str_has_suffix(path, "/..");
  g_free(root_prefix);
  return allowed;
}

bool mount_grant_in_sandbox(DocumentGrant *grant, const char *sandbox_doc_dir,
                            GError **error) {
  char *base = g_path_get_basename(grant->host_path);
  char *target_dir = g_build_filename(sandbox_doc_dir, grant->doc_id, NULL);
  char *target = g_build_filename(target_dir, base, NULL);
  g_free(base);
  g_free(target_dir);

  bool read_only = !g_strv_contains((const char *const *)grant->permissions,
                                   "write");
  if (!mount_grant_path(grant->host_path, target, read_only, error)) {
    g_free(target);
    return false;
  }
  g_ptr_array_add(grant->target_paths, target);
  return true;
}

void remove_sandbox_grants(BridgeState *state, const char *sandbox_doc_dir) {
  char *prefix = g_strconcat(sandbox_doc_dir, G_DIR_SEPARATOR_S, NULL);
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    for (guint j = grant->target_paths->len; j > 0; j--) {
      const char *target = g_ptr_array_index(grant->target_paths, j - 1);
      if (g_str_has_prefix(target, prefix)) {
        unmount_path(target);
        g_ptr_array_remove_index(grant->target_paths, j - 1);
      }
    }
  }
  g_free(prefix);
}
