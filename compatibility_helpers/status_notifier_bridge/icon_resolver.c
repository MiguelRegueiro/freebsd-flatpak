#include "icon_resolver.h"
#include <gdk-pixbuf/gdk-pixbuf.h>
#include <sys/stat.h>

static bool regular_file(const char *path) {
  struct stat info;
  return g_lstat(path, &info) == 0 && S_ISREG(info.st_mode);
}

static bool real_directory(const char *path) {
  struct stat info;
  return g_lstat(path, &info) == 0 && S_ISDIR(info.st_mode);
}

static bool supported_icon_file(const char *filename, const char *icon_name) {
  if (!g_str_has_prefix(filename, icon_name)) {
    return false;
  }
  const char *extension = filename + strlen(icon_name);
  return g_ascii_strcasecmp(extension, ".png") == 0 ||
         g_ascii_strcasecmp(extension, ".svg") == 0 ||
         g_ascii_strcasecmp(extension, ".xpm") == 0;
}

static char *path_within_root(const char *root, const char *path) {
  char *canonical_root = realpath(root, NULL);
  char *canonical_path = realpath(path, NULL);
  if (canonical_root == NULL || canonical_path == NULL) {
    free(canonical_root);
    free(canonical_path);
    return NULL;
  }
  size_t root_length = strlen(canonical_root);
  bool inside = g_str_has_prefix(canonical_path, canonical_root) &&
                (canonical_path[root_length] == '\0' ||
                 canonical_path[root_length] == G_DIR_SEPARATOR);
  free(canonical_root);
  if (!inside) {
    free(canonical_path);
    return NULL;
  }
  return canonical_path;
}

static void find_named_icons(const char *directory, const char *icon_name,
                             guint depth, GPtrArray *matches) {
  if (directory == NULL || depth > 8 || !real_directory(directory)) {
    return;
  }
  GError *error = NULL;
  GDir *dir = g_dir_open(directory, 0, &error);
  if (dir == NULL) {
    g_clear_error(&error);
    return;
  }
  const char *entry = NULL;
  while ((entry = g_dir_read_name(dir)) != NULL) {
    char *path = g_build_filename(directory, entry, NULL);
    if (real_directory(path)) {
      find_named_icons(path, icon_name, depth + 1, matches);
      g_free(path);
    } else if (regular_file(path) && supported_icon_file(entry, icon_name)) {
      g_ptr_array_add(matches, path);
    } else {
      g_free(path);
    }
  }
  g_dir_close(dir);
}

static char *translate_sandbox_path(StatusNotifierBridge *state,
                                    const char *path) {
  if (g_str_has_prefix(path, "/app/") && state->app_root != NULL) {
    char *candidate =
        g_build_filename(state->app_root, path + strlen("/app/"), NULL);
    char *translated = path_within_root(state->app_root, candidate);
    g_free(candidate);
    return translated;
  }
  if (g_strcmp0(path, "/app") == 0 && state->app_root != NULL) {
    return g_strdup(state->app_root);
  }
  if (g_str_has_prefix(path, "/usr/") && state->runtime_root != NULL) {
    char *candidate =
        g_build_filename(state->runtime_root, path + strlen("/usr/"), NULL);
    char *translated = path_within_root(state->runtime_root, candidate);
    g_free(candidate);
    return translated;
  }
  if (g_strcmp0(path, "/usr") == 0 && state->runtime_root != NULL) {
    return g_strdup(state->runtime_root);
  }
  return NULL;
}

static void add_search_root(GPtrArray *roots, char *path) {
  if (path != NULL && real_directory(path)) {
    g_ptr_array_add(roots, path);
  } else {
    g_free(path);
  }
}

static GPtrArray *icon_paths(StatusNotifierBridge *state, const char *icon_name,
                             const char *icon_theme_path) {
  GPtrArray *matches = g_ptr_array_new_with_free_func(g_free);
  if (icon_name == NULL || *icon_name == '\0') {
    return matches;
  }

  if (g_path_is_absolute(icon_name)) {
    char *path = translate_sandbox_path(state, icon_name);
    if (path != NULL && regular_file(path)) {
      g_ptr_array_add(matches, path);
    } else {
      g_free(path);
    }
    return matches;
  }
  if (strchr(icon_name, '/') != NULL) {
    return matches;
  }

  GPtrArray *roots = g_ptr_array_new_with_free_func(g_free);
  if (icon_theme_path != NULL && *icon_theme_path != '\0') {
    add_search_root(roots, translate_sandbox_path(state, icon_theme_path));
  }
  if (state->app_root != NULL) {
    add_search_root(roots,
                    g_build_filename(state->app_root, "share", "icons", NULL));
    add_search_root(
        roots, g_build_filename(state->app_root, "share", "pixmaps", NULL));
  }
  if (state->runtime_root != NULL) {
    add_search_root(
        roots,
        g_build_filename(state->runtime_root, "share", "icons", NULL));
    add_search_root(roots, g_build_filename(state->runtime_root, "share",
                                            "pixmaps", NULL));
  }
  for (guint i = 0; i < roots->len; i++) {
    find_named_icons(g_ptr_array_index(roots, i), icon_name, 0, matches);
    if (matches->len > 0) {
      break;
    }
  }
  g_ptr_array_free(roots, TRUE);
  return matches;
}

static GdkPixbuf *bounded_pixbuf(const char *path) {
  GError *error = NULL;
  GdkPixbuf *pixbuf = gdk_pixbuf_new_from_file(path, &error);
  if (pixbuf == NULL) {
    status_notifier_log("load icon %s failed: %s", path, error->message);
    g_error_free(error);
    return NULL;
  }
  int width = gdk_pixbuf_get_width(pixbuf);
  int height = gdk_pixbuf_get_height(pixbuf);
  if (width <= 128 && height <= 128) {
    return pixbuf;
  }
  double scale = MIN(128.0 / width, 128.0 / height);
  GdkPixbuf *scaled = gdk_pixbuf_scale_simple(
      pixbuf, MAX(1, (int)(width * scale)), MAX(1, (int)(height * scale)),
      GDK_INTERP_BILINEAR);
  g_object_unref(pixbuf);
  return scaled;
}

static void add_pixbuf(GVariantBuilder *builder, GdkPixbuf *pixbuf) {
  int width = gdk_pixbuf_get_width(pixbuf);
  int height = gdk_pixbuf_get_height(pixbuf);
  int channels = gdk_pixbuf_get_n_channels(pixbuf);
  int stride = gdk_pixbuf_get_rowstride(pixbuf);
  bool has_alpha = gdk_pixbuf_get_has_alpha(pixbuf);
  const guchar *pixels = gdk_pixbuf_read_pixels(pixbuf);
  gsize size = (gsize)width * (gsize)height * 4;
  guchar *argb = g_malloc(size);
  for (int y = 0; y < height; y++) {
    const guchar *row = pixels + y * stride;
    for (int x = 0; x < width; x++) {
      const guchar *source = row + x * channels;
      guchar *target = argb + ((gsize)y * width + x) * 4;
      target[0] = has_alpha ? source[3] : 0xff;
      target[1] = source[0];
      target[2] = source[1];
      target[3] = source[2];
    }
  }
  GVariant *bytes = g_variant_new_from_data(G_VARIANT_TYPE("ay"), argb, size,
                                             TRUE, g_free, argb);
  g_variant_builder_add(builder, "(ii@ay)", width, height, bytes);
}

GVariant *resolve_status_icon(StatusNotifierBridge *state,
                              const char *icon_name,
                              const char *icon_theme_path) {
  GPtrArray *paths = icon_paths(state, icon_name, icon_theme_path);
  if (paths->len == 0) {
    g_ptr_array_free(paths, TRUE);
    return NULL;
  }

  GVariantBuilder pixmaps;
  g_variant_builder_init(&pixmaps, G_VARIANT_TYPE("a(iiay)"));
  guint loaded = 0;
  for (guint i = 0; i < paths->len; i++) {
    GdkPixbuf *pixbuf = bounded_pixbuf(g_ptr_array_index(paths, i));
    if (pixbuf != NULL) {
      add_pixbuf(&pixmaps, pixbuf);
      g_object_unref(pixbuf);
      loaded++;
    }
  }
  g_ptr_array_free(paths, TRUE);
  if (loaded == 0) {
    g_variant_builder_clear(&pixmaps);
    return NULL;
  }
  return g_variant_ref_sink(g_variant_builder_end(&pixmaps));
}
