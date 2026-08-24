#include "document_mount_backend.h"

static bool run_argv(char **argv, GError **error) {
  gint status = 0;
  gchar *stderr_text = NULL;
  if (!g_spawn_sync(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL, NULL,
                    &stderr_text, &status, error)) {
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

bool mount_grant_path(const char *source, const char *target, bool read_only,
                      GError **error) {
  char *read_only_argv[] = {"doas",         "mount_nullfs", "-o", "ro",
                            (char *)source, (char *)target, NULL};
  char *writable_argv[] = {"doas", "mount_nullfs", (char *)source,
                           (char *)target, NULL};
  return run_argv(read_only ? read_only_argv : writable_argv, error);
}

bool unmount_path(const char *target) {
  GError *error = NULL;
  char *argv[] = {"doas", "umount", (char *)target, NULL};
  if (run_argv(argv, &error)) {
    return true;
  }
  if (error != NULL) {
    log_line("umount failed for %s: %s", target, error->message);
    g_error_free(error);
  }

  error = NULL;
  char *force_argv[] = {"doas", "umount", "-f", (char *)target, NULL};
  if (run_argv(force_argv, &error)) {
    return true;
  }
  if (error != NULL) {
    log_line("forced umount failed for %s: %s", target, error->message);
    g_error_free(error);
  }
  return false;
}
