#include "document_mount_backend.h"

#include <inttypes.h>

#define SECURE_MOUNT "/usr/local/libexec/freebsd-flatpak/secure-mount"

static bool run_argv(char **argv, GError **error) {
  gint status = 0;
  gchar *stderr_text = NULL;
  if (!g_spawn_sync(NULL, argv, NULL, 0, NULL, NULL, NULL, &stderr_text,
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

/* Find the exact instance root rather than trusting a D-Bus-provided target.
 * secure-mount repeats this validation as the privilege boundary; this keeps
 * the bridge responsible only for translating its document target into that
 * narrow helper's root-relative protocol. */
static bool secure_mount_target(const char *target, char **root_out,
                                char **relative_out, struct stat *root_stat,
                                GError **error) {
  char *canonical = g_canonicalize_filename(target, NULL);
  char *candidate = g_strdup(canonical);
  while (candidate != NULL && !g_str_equal(candidate, "/")) {
    char *info = g_build_filename(candidate, ".flatpak-info", NULL);
    bool found = g_file_test(info, G_FILE_TEST_IS_REGULAR);
    g_free(info);
    if (found) {
      gsize root_len = strlen(candidate);
      if (!g_str_has_prefix(canonical, candidate) ||
          canonical[root_len] != G_DIR_SEPARATOR) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                    "document target is outside sandbox root: %s", target);
        g_free(candidate);
        g_free(canonical);
        return false;
      }
      if (g_stat(candidate, root_stat) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                    "stat sandbox root %s failed: %s", candidate,
                    g_strerror(errno));
        g_free(candidate);
        g_free(canonical);
        return false;
      }
      *root_out = candidate;
      *relative_out = g_strdup(canonical + root_len + 1);
      g_free(canonical);
      return true;
    }
    char *parent = g_path_get_dirname(candidate);
    g_free(candidate);
    candidate = parent;
  }
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
              "document target is not below a sandbox root: %s", target);
  g_free(canonical);
  return false;
}

static bool run_secure_mount(const char *operation, const char *source,
                             const char *target, const char *access,
                             GError **error) {
  char *root = NULL;
  char *relative = NULL;
  struct stat root_stat;
  if (!secure_mount_target(target, &root, &relative, &root_stat, error)) {
    return false;
  }
  char *root_device = g_strdup_printf("%ju", (uintmax_t)root_stat.st_dev);
  char *root_inode = g_strdup_printf("%ju", (uintmax_t)root_stat.st_ino);
  bool ok = false;
  if (g_str_equal(operation, "nullfs")) {
    char *resolved_source = realpath(source, NULL);
    struct stat source_stat;
    if (resolved_source == NULL || g_stat(resolved_source, &source_stat) != 0) {
      g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                  "resolve document source %s failed: %s", source,
                  g_strerror(errno));
      free(resolved_source);
    } else {
      char *source_device =
          g_strdup_printf("%ju", (uintmax_t)source_stat.st_dev);
      char *source_inode =
          g_strdup_printf("%ju", (uintmax_t)source_stat.st_ino);
      char *argv[] = {SECURE_MOUNT, "nullfs", root, root_device, root_inode,
                      relative, resolved_source, source_device, source_inode,
                      (char *)access, NULL};
      ok = run_argv(argv, error);
      g_free(source_inode);
      g_free(source_device);
      free(resolved_source);
    }
  } else {
    char *argv[] = {SECURE_MOUNT, "unmount", root, root_device, root_inode,
                    relative, (char *)access, NULL};
    ok = run_argv(argv, error);
  }
  g_free(root_inode);
  g_free(root_device);
  g_free(relative);
  g_free(root);
  return ok;
}

bool mount_grant_path(const char *source, const char *target, bool read_only,
                      GError **error) {
  return run_secure_mount("nullfs", source, target,
                          read_only ? "ro" : "rw", error);
}

bool unmount_path(const char *target) {
  GError *error = NULL;
  if (run_secure_mount("unmount", NULL, target, "normal", &error)) {
    return true;
  }
  if (error != NULL) {
    log_line("umount failed for %s: %s", target, error->message);
    g_error_free(error);
  }

  error = NULL;
  if (run_secure_mount("unmount", NULL, target, "force", &error)) {
    return true;
  }
  if (error != NULL) {
    log_line("forced umount failed for %s: %s", target, error->message);
    g_error_free(error);
  }
  return false;
}
