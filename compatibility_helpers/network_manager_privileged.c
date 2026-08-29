/* Narrow set-user-ID backend for NetworkManager compatibility activation. */
#include <arpa/inet.h>
#include <gio/gio.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <net/if.h>
#include <signal.h>
#include <stdlib.h>
#include <stdio.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef STATE_PARENT
#define STATE_PARENT "/var/run/freebsd-flatpak"
#endif
#ifndef STATE_DIR
#define STATE_DIR "/var/run/freebsd-flatpak/network-manager"
#endif
#ifndef IFCONFIG
#define IFCONFIG "/sbin/ifconfig"
#endif
#ifndef WG
#define WG "/usr/bin/wg"
#endif
#ifndef ROUTE
#define ROUTE "/sbin/route"
#endif
#ifndef RESOLVCONF
#define RESOLVCONF "/sbin/resolvconf"
#endif
#define MAX_SETTINGS_INPUT (1024 * 1024)
#define WIREGUARD_RENAME_ATTEMPTS 16
#ifdef NM_PRIVILEGED_TESTING
#define STATE_OWNER_UID getuid()
#else
#define STATE_OWNER_UID 0
#endif
#ifndef FREEBSD_FLATPAK_OWNER_UID
#define FREEBSD_FLATPAK_OWNER_UID ((uid_t)-1)
#endif

typedef struct {
  gchar *token;
  gchar *interface_name;
  gchar *rename_interface_name;
  GPtrArray *routes;
  GPtrArray *endpoints;
  gboolean default_ipv4;
  gboolean default_ipv6;
  gboolean peer_routes;
  uid_t owner_uid;
  pid_t owner_pid;
  gboolean dns_installed;
}  Activation;

static gboolean save_state(Activation *activation, GError **error);

static void die(const char *message) { g_printerr("network-manager-privileged: %s\n", message); }

static gboolean run(char *const argv[], GError **error) {
  GSubprocess *child = g_subprocess_newv((const gchar * const *)argv,
      G_SUBPROCESS_FLAGS_STDOUT_SILENCE | G_SUBPROCESS_FLAGS_STDERR_PIPE, error);
  if (!child) return FALSE;
  GBytes *stdout_data = NULL, *stderr_data = NULL;
  gboolean communicated = g_subprocess_communicate(child, NULL, NULL, &stdout_data, &stderr_data, error);
  if (stdout_data) g_bytes_unref(stdout_data);
  if (!communicated) { if (stderr_data) g_bytes_unref(stderr_data); g_object_unref(child); return FALSE; }
  if (!g_subprocess_get_successful(child)) {
    gsize length = 0; const char *data = g_bytes_get_data(stderr_data, &length);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "%s failed: %.*s", argv[0], (int)length, data ?: "");
    g_bytes_unref(stderr_data); g_object_unref(child); return FALSE;
  }
  g_bytes_unref(stderr_data); g_object_unref(child); return TRUE;
}
static gboolean ensure_directory(const char *path, gboolean private, GError **error) {
  struct stat st;
  if (mkdir(path, private ? 0700 : 0755) != 0 && errno != EEXIST) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "create helper state directory: %s", g_strerror(errno));
    return FALSE;
  }
  if (lstat(path, &st) != 0 || !S_ISDIR(st.st_mode) || st.st_uid != STATE_OWNER_UID || (st.st_mode & 022) != 0 || (private && (st.st_mode & 077) != 0)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED, "unsafe helper state directory");
    return FALSE;
  }
  return TRUE;
}
static gboolean ensure_state_layout(GError **error) {
  return ensure_directory(STATE_PARENT, FALSE, error) && ensure_directory(STATE_DIR, TRUE, error);
}
static int acquire_state_lock(GError **error) {
  gchar *path = g_build_filename(STATE_DIR, ".lock", NULL);
  int fd = open(path, O_RDWR | O_CREAT | O_NOFOLLOW, 0600);
  g_free(path);
  if (fd < 0 || flock(fd, LOCK_EX) != 0) {
    int saved = errno;
    if (fd >= 0) close(fd);
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(saved), "lock helper state: %s", g_strerror(saved));
    return -1;
  }
  return fd;
}
static gboolean sanitize_environment(GError **error) {
  if (clearenv() != 0 || setenv("PATH", "/usr/sbin:/usr/bin:/sbin:/bin", 1) != 0 || setenv("LC_ALL", "C", 1) != 0) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "sanitize helper environment: %s", g_strerror(errno));
    return FALSE;
  }
  return TRUE;
}
static gboolean caller_authorized(GError **error) {
  if (geteuid() != 0 || getuid() == 0 || getuid() != FREEBSD_FLATPAK_OWNER_UID) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED, "caller is not authorized for NetworkManager activation");
    return FALSE;
  }
  return TRUE;
}
static gboolean pid_alive(pid_t pid) {
  if (pid <= 1) return FALSE;
  if (kill(pid, 0) == 0) return TRUE;
  return errno == EPERM;
}


static gboolean run_capture(char *const argv[], gchar **output, GError **error) {
  GSubprocess *child = g_subprocess_newv((const gchar * const *)argv,
      G_SUBPROCESS_FLAGS_STDOUT_PIPE | G_SUBPROCESS_FLAGS_STDERR_PIPE, error);
  if (!child) return FALSE;
  GBytes *stdout_data = NULL, *stderr_data = NULL;
  gboolean communicated = g_subprocess_communicate(child, NULL, NULL, &stdout_data, &stderr_data, error);
  if (!communicated) { g_clear_pointer(&stdout_data, g_bytes_unref); g_clear_pointer(&stderr_data, g_bytes_unref); g_object_unref(child); return FALSE; }
  if (!g_subprocess_get_successful(child)) {
    gsize length = 0; const char *data = g_bytes_get_data(stderr_data, &length);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "%s failed: %.*s", argv[0], (int)length, data ?: "");
    g_bytes_unref(stdout_data); g_bytes_unref(stderr_data); g_object_unref(child); return FALSE;
  }
  gsize length = 0; const char *data = stdout_data ? g_bytes_get_data(stdout_data, &length) : NULL;
  *output = data ? g_strndup(data, length) : g_strdup(""); g_strchomp(*output);
  if (stdout_data) g_bytes_unref(stdout_data); g_bytes_unref(stderr_data); g_object_unref(child); return TRUE;
}
static gboolean run_input(char *const argv[], const char *input, GError **error) {
  GSubprocess *child = g_subprocess_newv((const gchar * const *)argv,
      G_SUBPROCESS_FLAGS_STDIN_PIPE | G_SUBPROCESS_FLAGS_STDOUT_SILENCE | G_SUBPROCESS_FLAGS_STDERR_PIPE, error);
  if (!child) return FALSE;
  GBytes *stdout_data = NULL, *stderr_data = NULL;
  GBytes *payload = g_bytes_new(input, strlen(input));
  gboolean communicated = g_subprocess_communicate(child, payload, NULL, &stdout_data, &stderr_data, error);
  g_bytes_unref(payload); g_clear_pointer(&stdout_data, g_bytes_unref);
  if (!communicated) { g_clear_pointer(&stderr_data, g_bytes_unref); g_object_unref(child); return FALSE; }
  if (!g_subprocess_get_successful(child)) {
    gsize length = 0; const char *data = g_bytes_get_data(stderr_data, &length);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "%s failed: %.*s", argv[0], (int)length, data ?: "");
    g_bytes_unref(stderr_data); g_object_unref(child); return FALSE;
  }
  g_bytes_unref(stderr_data); g_object_unref(child); return TRUE;
}

static GVariant *value(GVariant *settings, const char *section, const char *key) {
  GVariant *fields = g_variant_lookup_value(settings, section, G_VARIANT_TYPE("a{sv}"));
  if (!fields) return NULL;
  GVariant *field = g_variant_lookup_value(fields, key, NULL);
  g_variant_unref(fields); return field;
}
static gchar *string_value(GVariant *settings, const char *section, const char *key) {
  GVariant *field = value(settings, section, key);
  if (!field) return NULL;
  gchar *result = g_variant_is_of_type(field, G_VARIANT_TYPE_STRING) ? g_variant_dup_string(field, NULL) : NULL;
  g_variant_unref(field); return result;
}
static guint32 uint_value(GVariant *settings, const char *section, const char *key) {
  GVariant *field = value(settings, section, key); guint32 result = 0;
  if (field && g_variant_is_of_type(field, G_VARIANT_TYPE_UINT32)) result = g_variant_get_uint32(field);
  if (field) g_variant_unref(field); return result;
}
static gboolean bool_value_or(GVariant *settings, const char *section, const char *key,
                              gboolean fallback, GError **error) {
  GVariant *field = value(settings, section, key);
  if (!field) return fallback;
  if (!g_variant_is_of_type(field, G_VARIANT_TYPE_BOOLEAN)) {
    g_variant_unref(field);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s.%s", section, key);
    return FALSE;
  }
  gboolean result = g_variant_get_boolean(field);
  g_variant_unref(field);
  return result;
}
static gchar *key_value(GVariant *fields, const char *key) {
  GVariant *field = g_variant_lookup_value(fields, key, NULL);
  if (!field) return NULL;
  gchar *result = NULL;
  if (g_variant_is_of_type(field, G_VARIANT_TYPE("ay"))) {
    gsize length = 0; const guint8 *data = g_variant_get_fixed_array(field, &length, 1);
    if (length == 32) result = g_base64_encode(data, length);
  } else if (g_variant_is_of_type(field, G_VARIANT_TYPE_STRING)) result = g_variant_dup_string(field, NULL);
  g_variant_unref(field); return result;
}
static gboolean valid_name(const char *name) {
  if (!name || !*name || strlen(name) >= IFNAMSIZ) return FALSE;
  for (const char *p = name; *p; p++) if (!g_ascii_isalnum(*p)) return FALSE;
  return TRUE;
}
static gboolean valid_wireguard_clone_name(const char *name) {
  if (!valid_name(name) || !g_str_has_prefix(name, "wg") || !g_ascii_isdigit(name[2])) return FALSE;
  for (const char *p = name + 2; *p; p++) if (!g_ascii_isdigit(*p)) return FALSE;
  return TRUE;
}
/* FreeBSD's WireGuard cloner only accepts wgN names.  Create one through the
 * cloner, then give the activation its private managed name. */
static gchar *new_wireguard_clone_name(void) {
  return g_strdup_printf("wg%u", arc4random_uniform(1000));
}
static gchar *new_epair_name(void) {
  return g_strdup_printf("epair%ua", arc4random_uniform(1000));
}
static gboolean create_dummy_interface(Activation *activation, GError **error) {
  gchar *clone = g_strndup(activation->interface_name, strlen(activation->interface_name) - 1);
  char *create[] = { (char *)IFCONFIG, clone, "create", NULL };
  gchar *created = NULL;
  gboolean ok = run_capture(create, &created, error);
  if (ok && !g_str_equal(created, activation->interface_name)) {
    if (valid_name(created) && g_str_has_prefix(created, "epair")) { char *destroy[] = { (char *)IFCONFIG, created, "destroy", NULL }; GError *ignored = NULL; run(destroy, &ignored); g_clear_error(&ignored); }
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "invalid epair name returned by ifconfig"); ok = FALSE;
  }
  g_free(created); g_free(clone); return ok;
}

static gboolean create_wireguard_interface(Activation *activation, GError **error) {
  if (!valid_wireguard_clone_name(activation->interface_name)) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard clone name"); return FALSE; }
  gchar *clone_name = g_strdup(activation->interface_name);
  char *create[] = { (char *)IFCONFIG, clone_name, "create", NULL };
  if (!run(create, error)) { g_free(clone_name); return FALSE; }
  GError *last_error = NULL;
  for (guint attempt = 0; attempt < WIREGUARD_RENAME_ATTEMPTS; attempt++) {
    gchar *managed_name = g_strdup_printf("fwg%08x", arc4random());
    g_free(activation->rename_interface_name);
    activation->rename_interface_name = g_strdup(clone_name);
    g_free(activation->interface_name);
    activation->interface_name = managed_name;
    if (!save_state(activation, error)) { g_free(clone_name); return FALSE; }
    char *rename[] = { (char *)IFCONFIG, activation->rename_interface_name, "name", activation->interface_name, NULL };
    GError *rename_error = NULL;
    if (run(rename, &rename_error)) {
      g_free(activation->rename_interface_name); activation->rename_interface_name = NULL;
      gboolean saved = save_state(activation, error);
      g_free(clone_name);
      return saved;
    }
    g_clear_error(&last_error); last_error = rename_error;
  }
  g_free(clone_name);
  g_propagate_prefixed_error(error, last_error, "could not assign a private WireGuard interface name after %u attempts: ", WIREGUARD_RENAME_ATTEMPTS);
  return FALSE;
}
static gboolean valid_token(const char *token) {
  if (!token || strlen(token) != 32) return FALSE;
  for (const char *p = token; *p; p++) if (!g_ascii_isxdigit(*p)) return FALSE;
  return TRUE;
}
static gchar *new_token(void) {
  guint8 bytes[16];
  arc4random_buf(bytes, sizeof(bytes));
  GString *token = g_string_sized_new(32);
  for (guint index = 0; index < G_N_ELEMENTS(bytes); index++) g_string_append_printf(token, "%02x", bytes[index]);
  return g_string_free(token, FALSE);
}
static gboolean write_secret_key(const char *key, gchar **path, GError **error) {
  gchar *template = g_strdup(STATE_DIR "/key.XXXXXX"); int fd = g_mkstemp_full(template, O_WRONLY, 0600);
  if (fd < 0) { g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "create WireGuard key file: %s", g_strerror(errno)); g_free(template); return FALSE; }
  gboolean ok = write(fd, key, strlen(key)) == (ssize_t)strlen(key) && write(fd, "\n", 1) == 1;
  if (close(fd) != 0) ok = FALSE;
  if (!ok) { g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "write WireGuard key file: %s", g_strerror(errno)); unlink(template); g_free(template); return FALSE; }
  *path = template; return TRUE;
}
static gboolean configure_addresses(GVariant *settings, const char *section, int family, const char *interface_name, GError **error) {
  const char *method = string_value(settings, section, "method");
  if (method && (g_str_equal(method, "disabled") || g_str_equal(method, "ignore"))) { g_free((gpointer)method); return TRUE; }
  g_free((gpointer)method);
  GVariant *addresses = value(settings, section, "address-data");
  if (!addresses) return TRUE;
  GVariantIter iter; GVariant *address;
  g_variant_iter_init(&iter, addresses);
  while ((address = g_variant_iter_next_value(&iter))) {
    gchar *ip = string_value(address, "", "");
    GVariant *raw_ip = g_variant_lookup_value(address, "address", G_VARIANT_TYPE_STRING);
    gchar *ip_text = raw_ip ? g_variant_dup_string(raw_ip, NULL) : NULL;
    if (raw_ip) g_variant_unref(raw_ip);
    GVariant *raw_prefix = g_variant_lookup_value(address, "prefix", G_VARIANT_TYPE_UINT32);
    guint32 prefix = raw_prefix ? g_variant_get_uint32(raw_prefix) : 0;
    if (raw_prefix) g_variant_unref(raw_prefix);
    if (!ip_text || prefix > (family == AF_INET ? 32 : 128)) { g_free(ip); g_free(ip_text); g_variant_unref(address); g_variant_unref(addresses); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s address-data", section); return FALSE; }
    gchar *cidr = g_strdup_printf("%s/%u", ip_text, prefix);
    char *argv[] = { (char *)IFCONFIG, (char *)interface_name, family == AF_INET ? "inet" : "inet6", cidr, "alias", NULL };
    gboolean ok = run(argv, error); g_free(ip); g_free(ip_text); g_free(cidr); g_variant_unref(address);
    if (!ok) { g_variant_unref(addresses); return FALSE; }
  }
  g_variant_unref(addresses); return TRUE;
}

static gboolean dns_contents(GVariant *settings, gboolean include_ipv4, gboolean include_ipv6, GString **contents, GError **error) {
  GString *result = g_string_new(NULL);
  for (const char *section = "ipv4"; section; section = g_str_equal(section, "ipv4") ? "ipv6" : NULL) {
    gboolean include = g_str_equal(section, "ipv4") ? include_ipv4 : include_ipv6;
    if (!include) continue;
    int family = g_str_equal(section, "ipv4") ? AF_INET : AF_INET6;
    GVariant *data = value(settings, section, "dns-data");
    if (data) {
      if (!g_variant_is_of_type(data, G_VARIANT_TYPE("aa{sv}"))) { g_variant_unref(data); g_string_free(result, TRUE); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s.dns-data", section); return FALSE; }
      GVariantIter iter; GVariant *entry; g_variant_iter_init(&iter, data);
      while ((entry = g_variant_iter_next_value(&iter))) {
        GVariant *field = g_variant_lookup_value(entry, "address", G_VARIANT_TYPE_STRING);
        gchar *address = field ? g_variant_dup_string(field, NULL) : NULL;
        if (field) g_variant_unref(field); g_variant_unref(entry);
        guint8 parsed[sizeof(struct in6_addr)];
        if (!address || inet_pton(family, address, parsed) != 1) { g_free(address); g_variant_unref(data); g_string_free(result, TRUE); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s.dns-data", section); return FALSE; }
        g_string_append_printf(result, "nameserver %s\n", address);
        g_free(address);
      }
      g_variant_unref(data);
    }
    GVariant *legacy = value(settings, section, "dns");
    if (legacy) {
      if (family != AF_INET || !g_variant_is_of_type(legacy, G_VARIANT_TYPE("au"))) { g_variant_unref(legacy); g_string_free(result, TRUE); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s.dns", section); return FALSE; }
      GVariantIter iter; guint32 number; g_variant_iter_init(&iter, legacy);
      while (g_variant_iter_next(&iter, "u", &number)) {
        struct in_addr address = { .s_addr = htonl(number) }; char text[INET_ADDRSTRLEN];
        if (!inet_ntop(AF_INET, &address, text, sizeof(text))) { g_variant_unref(legacy); g_string_free(result, TRUE); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid %s.dns", section); return FALSE; }
        g_string_append_printf(result, "nameserver %s\n", text);
      }
      g_variant_unref(legacy);
    }
  }
  *contents = result; return TRUE;
}
static gboolean configure_dns(Activation *activation, GVariant *settings, GError **error) {
  if (!activation->default_ipv4 && !activation->default_ipv6) return TRUE;
  GString *contents = NULL;
  if (!dns_contents(settings, activation->default_ipv4, activation->default_ipv6, &contents, error)) return FALSE;
  if (!contents->len) { g_string_free(contents, TRUE); g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED, "full-tunnel WireGuard connection requires DNS settings for a default-routed family"); return FALSE; }
  gchar *name = g_strdup_printf("%s.wireguard", activation->interface_name);
  char *argv[] = { (char *)RESOLVCONF, "-x", "-a", name, NULL };
  gboolean ok = run_input(argv, contents->str, error);
  g_free(name); g_string_free(contents, TRUE);
  if (ok) activation->dns_installed = TRUE;
  return ok;
}
static gboolean remove_dns(const char *interface_name, GError **error) {
  gchar *name = g_strdup_printf("%s.wireguard", interface_name);
  char *argv[] = { (char *)RESOLVCONF, "-f", "-d", name, NULL };
  gboolean ok = run(argv, error);
  g_free(name); return ok;
}
static gboolean peer_keepalive(GVariant *peer, guint16 *keepalive, GError **error) {
  GVariant *field = g_variant_lookup_value(peer, "persistent-keepalive", NULL);
  if (!field) return TRUE;
  guint32 value = 0;
  if (g_variant_is_of_type(field, G_VARIANT_TYPE_UINT32)) value = g_variant_get_uint32(field);
  else if (g_variant_is_of_type(field, G_VARIANT_TYPE_UINT16)) value = g_variant_get_uint16(field);
  else { g_variant_unref(field); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard persistent-keepalive"); return FALSE; }
  g_variant_unref(field);
  if (value > G_MAXUINT16) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard persistent-keepalive"); return FALSE; }
  *keepalive = value;
  return TRUE;
}
/* NetworkManager serializes wireguard.peer allowed-ips as an array of CIDR strings. */
static gboolean peer_allowed_ips(GVariant *peer, GString **allowed_text, gboolean *default_ipv4, gboolean *default_ipv6, GError **error) {
  GVariant *allowed = g_variant_lookup_value(peer, "allowed-ips", NULL);
  GString *text = g_string_new(NULL);
  if (!allowed) { *allowed_text = text; return TRUE; }
  if (!g_variant_is_of_type(allowed, G_VARIANT_TYPE_STRING_ARRAY)) {
    g_variant_unref(allowed); g_string_free(text, TRUE);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard allowed-ips");
    return FALSE;
  }
  gsize count = 0;
  gchar **ips = g_variant_dup_strv(allowed, &count);
  g_variant_unref(allowed);
  for (gsize index = 0; index < count; index++) {
    if (!ips[index] || !*ips[index]) {
      g_strfreev(ips); g_string_free(text, TRUE);
      g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard allowed-ips");
      return FALSE;
    }
    if (g_str_equal(ips[index], "0.0.0.0/0")) *default_ipv4 = TRUE;
    if (g_str_equal(ips[index], "::/0")) *default_ipv6 = TRUE;
    if (text->len) g_string_append_c(text, ',');
    g_string_append(text, ips[index]);
  }
  g_strfreev(ips); *allowed_text = text; return TRUE;
}
static gboolean configure_peer(Activation *activation, GVariant *peer, GError **error) {
  gchar *public_key = key_value(peer, "public-key");
  gchar *endpoint = key_value(peer, "endpoint");
  gchar *preshared_key = key_value(peer, "preshared-key");
  GString *allowed_text = NULL;
  guint16 keepalive = 0;
  if (!public_key || !peer_allowed_ips(peer, &allowed_text, &activation->default_ipv4, &activation->default_ipv6, error) || !peer_keepalive(peer, &keepalive, error)) {
    g_free(public_key); g_free(endpoint); g_free(preshared_key); if (allowed_text) g_string_free(allowed_text, TRUE); if (!*error) g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "WireGuard peer needs public-key"); return FALSE;
  }
  gchar *preshared_key_file = NULL;
  if (preshared_key && !write_secret_key(preshared_key, &preshared_key_file, error)) {
    g_free(public_key); g_free(endpoint); g_free(preshared_key); g_string_free(allowed_text, TRUE); return FALSE;
  }
  gchar *keepalive_text = keepalive ? g_strdup_printf("%u", keepalive) : NULL;
  GPtrArray *argv = g_ptr_array_new();
  g_ptr_array_add(argv, (gpointer)WG); g_ptr_array_add(argv, "set"); g_ptr_array_add(argv, (gpointer)activation->interface_name); g_ptr_array_add(argv, "peer"); g_ptr_array_add(argv, public_key);
  if (endpoint) { g_ptr_array_add(argv, "endpoint"); g_ptr_array_add(argv, endpoint); g_ptr_array_add(activation->endpoints, g_strdup(endpoint)); }
  if (allowed_text->len) { g_ptr_array_add(argv, "allowed-ips"); g_ptr_array_add(argv, allowed_text->str); }
  if (keepalive_text) { g_ptr_array_add(argv, "persistent-keepalive"); g_ptr_array_add(argv, keepalive_text); }
  if (preshared_key_file) { g_ptr_array_add(argv, "preshared-key"); g_ptr_array_add(argv, preshared_key_file); }
  g_ptr_array_add(argv, NULL);
  gboolean ok = run((char *const *)argv->pdata, error);
  if (preshared_key_file) unlink(preshared_key_file);
  g_ptr_array_unref(argv); g_free(keepalive_text); g_free(preshared_key_file); g_free(public_key); g_free(endpoint); g_free(preshared_key); g_string_free(allowed_text, TRUE);
  return ok;
}
static gboolean endpoint_host(const char *endpoint, gchar **host, GError **error) {
  if (!endpoint || !*endpoint) goto invalid;
  const char *port = NULL;
  if (g_str_has_prefix(endpoint, "[")) {
    const char *close = strchr(endpoint, "]"[0]);
    if (!close || close == endpoint + 1 || close[1] != ":"[0]) goto invalid;
    *host = g_strndup(endpoint + 1, close - endpoint - 1);
    port = close + 2;
  } else {
    const char *colon = strrchr(endpoint, ":"[0]);
    if (!colon || colon == endpoint || strchr(endpoint, ":"[0]) != colon) goto invalid;
    *host = g_strndup(endpoint, colon - endpoint);
    port = colon + 1;
  }
  char *end = NULL;
  errno = 0;
  unsigned long number = strtoul(port, &end, 10);
  if (errno || !*port || *end || number == 0 || number > 65535) { g_free(*host); *host = NULL; goto invalid; }
  return TRUE;
invalid:
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid WireGuard endpoint");
  return FALSE;
}
static gboolean valid_route_address(int family, const char *address) {
  if (!address || !*address) return FALSE;
  gchar *copy = g_strdup(address);
  gchar *scope = strchr(copy, "%"[0]);
  if (scope) {
    *scope++ = 0;
    if (family != AF_INET6 || !valid_name(scope)) { g_free(copy); return FALSE; }
  }
  guint8 parsed[sizeof(struct in6_addr)];
  gboolean valid = inet_pton(family, copy, parsed) == 1;
  g_free(copy);
  return valid;
}
static gboolean endpoint_addresses(const char *endpoint, int family, GPtrArray *addresses, GError **error) {
  gchar *host = NULL;
  if (!endpoint_host(endpoint, &host, error)) return FALSE;
  guint8 parsed[sizeof(struct in6_addr)];
  if (inet_pton(family, host, parsed) == 1) { g_ptr_array_add(addresses, host); return TRUE; }
  struct in_addr other4; struct in6_addr other6;
  if (inet_pton(family == AF_INET ? AF_INET6 : AF_INET, host, family == AF_INET ? (void *)&other6 : (void *)&other4) == 1) { g_free(host); return TRUE; }
  struct addrinfo hints = { .ai_family = family, .ai_socktype = SOCK_DGRAM }, *resolved = NULL;
  int status = getaddrinfo(host, NULL, &hints, &resolved);
  g_free(host);
  if (status == EAI_NONAME || status == EAI_NODATA) return TRUE;
  if (status != 0 || !resolved) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "cannot resolve WireGuard endpoint"); return FALSE; }
  for (struct addrinfo *item = resolved; item; item = item->ai_next) {
    char text[INET6_ADDRSTRLEN];
    const void *source = family == AF_INET ? (const void *)&((struct sockaddr_in *)item->ai_addr)->sin_addr : (const void *)&((struct sockaddr_in6 *)item->ai_addr)->sin6_addr;
    if (!inet_ntop(family, source, text, sizeof(text))) { freeaddrinfo(resolved); g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "cannot resolve WireGuard endpoint"); return FALSE; }
    gboolean seen = FALSE;
    for (guint index = 0; index < addresses->len; index++) if (g_str_equal(g_ptr_array_index(addresses, index), text)) { seen = TRUE; break; }
    if (!seen) g_ptr_array_add(addresses, g_strdup(text));
  }
  freeaddrinfo(resolved);
  return TRUE;
}
static gboolean endpoint_gateway(int family, const char *endpoint, gchar **gateway, gboolean *already_pinned, GError **error) {
  const char *route_family = family == AF_INET ? "-inet" : "-inet6";
  char *argv[] = { (char *)ROUTE, "-n", "get", (char *)route_family, (char *)endpoint, NULL };
  gchar *output = NULL;
  if (!run_capture(argv, &output, error)) return FALSE;
  gchar **lines = g_strsplit(output, "\n", -1);
  for (guint index = 0; lines[index]; index++) {
    gchar *line = g_strstrip(lines[index]);
    if (g_str_has_prefix(line, "destination:")) {
      gchar *destination = g_strstrip(line + strlen("destination:"));
      if (g_str_equal(destination, endpoint)) *already_pinned = TRUE;
    } else if (g_str_has_prefix(line, "gateway:")) {
      gchar *candidate = g_strstrip(line + strlen("gateway:"));
      if (valid_route_address(family, candidate)) { g_free(*gateway); *gateway = g_strdup(candidate); }
    }
  }
  g_strfreev(lines); g_free(output);
  if (*already_pinned) { g_clear_pointer(gateway, g_free); return TRUE; }
  if (!*gateway) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "WireGuard endpoint has no routable gateway"); return FALSE; }
  return TRUE;
}
static gboolean add_endpoint_route(Activation *activation, int family, const char *endpoint, GError **error) {
  const char *family_text = family == AF_INET ? "4" : "6";
  const char *route_family = family == AF_INET ? "-inet" : "-inet6";
  for (guint index = 0; index < activation->routes->len; index++) {
    gchar **fields = g_strsplit(g_ptr_array_index(activation->routes, index), "\t", 3);
    gboolean found = fields[1] && fields[2] && g_str_equal(fields[0], family_text) && g_str_equal(fields[1], endpoint);
    g_strfreev(fields);
    if (found) return TRUE;
  }
  gchar *gateway = NULL;
  gboolean already_pinned = FALSE;
  if (!endpoint_gateway(family, endpoint, &gateway, &already_pinned, error)) return FALSE;
  if (already_pinned) return TRUE;
  char *argv[] = { (char *)ROUTE, "-n", "add", (char *)route_family, "-host", (char *)endpoint, gateway, NULL };
  g_ptr_array_add(activation->routes, g_strdup_printf("%s\t%s\t%s", family_text, endpoint, gateway));
  if (!save_state(activation, error)) { g_ptr_array_remove_index(activation->routes, activation->routes->len - 1); g_free(gateway); return FALSE; }
  gboolean ok = run(argv, error);
  g_free(gateway); return ok;
}
static gboolean pin_endpoints(Activation *activation, int family, GError **error) {
  for (guint index = 0; index < activation->endpoints->len; index++) {
    GPtrArray *addresses = g_ptr_array_new_with_free_func(g_free);
    gboolean ok = endpoint_addresses(g_ptr_array_index(activation->endpoints, index), family, addresses, error);
    for (guint address = 0; ok && address < addresses->len; address++) ok = add_endpoint_route(activation, family, g_ptr_array_index(addresses, address), error);
    g_ptr_array_unref(addresses);
    if (!ok) return FALSE;
  }
  return TRUE;
}
static gboolean configure_full_tunnel(Activation *activation, GError **error) {
  if (!activation->peer_routes || (!activation->default_ipv4 && !activation->default_ipv6)) return TRUE;
  if (!activation->endpoints->len) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "default WireGuard route requires a peer endpoint"); return FALSE; }
  if (activation->default_ipv4) {
    if (!pin_endpoints(activation, AF_INET, error)) return FALSE;
    char *lower[] = { (char *)ROUTE, "-n", "add", "-inet", "-net", "0.0.0.0/1", "-iface", activation->interface_name, NULL };
    char *upper[] = { (char *)ROUTE, "-n", "add", "-inet", "-net", "128.0.0.0/1", "-iface", activation->interface_name, NULL };
    if (!run(lower, error) || !run(upper, error)) return FALSE;
  }
  if (activation->default_ipv6) {
    if (!pin_endpoints(activation, AF_INET6, error)) return FALSE;
    char *lower[] = { (char *)ROUTE, "-n", "add", "-inet6", "-net", "::/1", "-iface", activation->interface_name, NULL };
    char *upper[] = { (char *)ROUTE, "-n", "add", "-inet6", "-net", "8000::/1", "-iface", activation->interface_name, NULL };
    if (!run(lower, error) || !run(upper, error)) return FALSE;
  }
  return TRUE;
}
static gboolean configure_wireguard(Activation *activation, GVariant *settings, GError **error) {
  GVariant *section = g_variant_lookup_value(settings, "wireguard", G_VARIANT_TYPE("a{sv}"));
  if (!section) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "WireGuard connection requires wireguard settings"); return FALSE; }
  activation->peer_routes = bool_value_or(settings, "wireguard", "peer-routes", TRUE, error);
  if (*error) { g_variant_unref(section); return FALSE; }
  gchar *private_key = key_value(section, "private-key");
  if (!private_key) { g_variant_unref(section); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "WireGuard connection requires private-key"); return FALSE; }
  gchar *key_file = NULL; if (!write_secret_key(private_key, &key_file, error)) { g_free(private_key); g_variant_unref(section); return FALSE; }
  char *key_argv[] = { (char *)WG, "set", activation->interface_name, "private-key", key_file, NULL };
  gboolean ok = run(key_argv, error); unlink(key_file); g_free(key_file); g_free(private_key); if (!ok) { g_variant_unref(section); return FALSE; }
  GVariant *peers = g_variant_lookup_value(section, "peers", G_VARIANT_TYPE("aa{sv}"));
  g_variant_unref(section);
  if (!peers) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "WireGuard connection requires wireguard.peers"); return FALSE; }
  GVariantIter iter; GVariant *peer; g_variant_iter_init(&iter, peers);
  while ((peer = g_variant_iter_next_value(&iter))) { ok = configure_peer(activation, peer, error); g_variant_unref(peer); if (!ok) { g_variant_unref(peers); return FALSE; } }
  g_variant_unref(peers);
  gboolean never_default_ipv4 = bool_value_or(settings, "ipv4", "never-default", FALSE, error);
  if (*error) return FALSE;
  gboolean never_default_ipv6 = bool_value_or(settings, "ipv6", "never-default", FALSE, error);
  if (*error) return FALSE;
  if (!activation->peer_routes || never_default_ipv4) activation->default_ipv4 = FALSE;
  if (!activation->peer_routes || never_default_ipv6) activation->default_ipv6 = FALSE;
  guint32 mtu = uint_value(settings, "wireguard", "mtu");
  if (mtu) { gchar *text = g_strdup_printf("%u", mtu); char *argv[] = { (char *)IFCONFIG, activation->interface_name, "mtu", text, NULL }; ok = run(argv, error); g_free(text); if (!ok) return FALSE; }
  return configure_addresses(settings, "ipv4", AF_INET, activation->interface_name, error) && configure_addresses(settings, "ipv6", AF_INET6, activation->interface_name, error);
}
typedef struct {
  gchar *interface_name;
  gchar *rename_interface_name;
  GPtrArray *routes;
  uid_t owner_uid;
  pid_t owner_pid;
} StoredActivation;
static void stored_activation_clear(StoredActivation *state) {
  g_free(state->interface_name);
  g_free(state->rename_interface_name);
  if (state->routes) g_ptr_array_unref(state->routes);
  memset(state, 0, sizeof(*state));
}
static gboolean write_all(int fd, const char *data, gsize length) {
  while (length) {
    ssize_t written = write(fd, data, length);
    if (written <= 0) return FALSE;
    data += written;
    length -= written;
  }
  return TRUE;
}
static gboolean save_state(Activation *activation, GError **error) {
  gchar *path = g_build_filename(STATE_DIR, activation->token, NULL);
  GString *body = g_string_new("v1\n");
  g_string_append_printf(body, "owner\t%u\t%ld\n", (unsigned)activation->owner_uid, (long)activation->owner_pid);
  g_string_append_printf(body, "interface\t%s\n", activation->interface_name);
  if (activation->rename_interface_name) g_string_append_printf(body, "rename\t%s\n", activation->rename_interface_name);
  for (guint index = 0; index < activation->routes->len; index++)
    g_string_append_printf(body, "route\t%s\n", (char *)g_ptr_array_index(activation->routes, index));
  gchar *temporary = g_strdup_printf("%s/.%s.%08x.tmp", STATE_DIR, activation->token, arc4random());
  int fd = open(temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
  if (fd < 0) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "create activation state: %s", g_strerror(errno));
    g_free(temporary); g_string_free(body, TRUE); g_free(path); return FALSE;
  }
  gboolean ok = write_all(fd, body->str, body->len) && fsync(fd) == 0;
  if (close(fd) != 0) ok = FALSE;
  if (ok && rename(temporary, path) != 0) ok = FALSE;
  int saved_errno = errno;
  if (!ok) {
    unlink(temporary);
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(saved_errno ?: EIO), "write activation state: %s", g_strerror(saved_errno ?: EIO));
  } else {
    int dirfd = open(STATE_DIR, O_RDONLY | O_DIRECTORY);
    if (dirfd >= 0) { fsync(dirfd); close(dirfd); }
  }
  g_free(temporary); g_string_free(body, TRUE); g_free(path); return ok;
}
static gboolean read_state_file(const char *token, gchar **contents, GError **error) {
  if (!valid_token(token)) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT, "invalid activation token"); return FALSE; }
  gchar *path = g_build_filename(STATE_DIR, token, NULL);
  int fd = open(path, O_RDONLY | O_NOFOLLOW);
  g_free(path);
  struct stat st;
  if (fd < 0 || fstat(fd, &st) != 0 || st.st_uid != STATE_OWNER_UID || (st.st_mode & 077) != 0 || !S_ISREG(st.st_mode) || st.st_size < 1 || st.st_size > MAX_SETTINGS_INPUT) {
    int saved = errno;
    if (fd >= 0) close(fd);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "activation state unavailable: %s", g_strerror(saved ?: EINVAL));
    return FALSE;
  }
  *contents = g_malloc(st.st_size + 1);
  gsize offset = 0;
  while (offset < (gsize)st.st_size) {
    ssize_t count = read(fd, *contents + offset, st.st_size - offset);
    if (count <= 0) { int saved = errno; close(fd); g_free(*contents); *contents = NULL; g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(saved ?: EIO), "read activation state: %s", g_strerror(saved ?: EIO)); return FALSE; }
    offset += count;
  }
  close(fd); (*contents)[offset] = 0; return TRUE;
}
static gboolean load_state(const char *token, StoredActivation *state, GError **error) {
  gchar *contents = NULL;
  if (!read_state_file(token, &contents, error)) return FALSE;
  gchar **lines = g_strsplit(contents, "\n", -1);
  gboolean valid = lines[0] && g_str_equal(lines[0], "v1") && lines[1] && lines[2];
  gchar **owner = valid ? g_strsplit(lines[1], "\t", 3) : NULL;
  gchar **interface = valid ? g_strsplit(lines[2], "\t", 2) : NULL;
  guint64 uid = 0, pid = 0;
  if (!owner || !interface || !owner[0] || !owner[1] || !owner[2] || !interface[0] || !interface[1] || !g_str_equal(owner[0], "owner") || !g_str_equal(interface[0], "interface")) valid = FALSE;
  if (valid) {
    char *end = NULL; errno = 0; uid = g_ascii_strtoull(owner[1], &end, 10); valid = !errno && end && !*end && uid != 0 && uid <= G_MAXUINT32;
    end = NULL; errno = 0; pid = g_ascii_strtoull(owner[2], &end, 10); valid = valid && !errno && end && !*end && pid > 1 && pid <= G_MAXINT;
    valid = valid && valid_name(interface[1]) && (g_str_has_prefix(interface[1], "fwg") || g_str_has_prefix(interface[1], "epair"));
  }
  state->routes = g_ptr_array_new_with_free_func(g_free);
  if (valid) state->interface_name = g_strdup(interface[1]);
  guint route_index = 3;
  if (valid && lines[route_index] && g_str_has_prefix(lines[route_index], "rename\t")) {
    gchar **rename = g_strsplit(lines[route_index], "\t", 2);
    if (!rename[0] || !rename[1] || !valid_wireguard_clone_name(rename[1])) valid = FALSE;
    else state->rename_interface_name = g_strdup(rename[1]);
    g_strfreev(rename); route_index++;
  }
  for (guint index = route_index; valid && lines[index] && *lines[index]; index++) {
    gchar **fields = g_strsplit(lines[index], "\t", 4);
    int family = fields[1] && g_str_equal(fields[1], "4") ? AF_INET : AF_INET6;
    if (!fields[0] || !fields[1] || !fields[2] || !fields[3] || !g_str_equal(fields[0], "route") || (!g_str_equal(fields[1], "4") && !g_str_equal(fields[1], "6")) || !valid_route_address(family, fields[2]) || !valid_route_address(family, fields[3])) valid = FALSE;
    else g_ptr_array_add(state->routes, g_strdup_printf("%s\t%s\t%s", fields[1], fields[2], fields[3]));
    g_strfreev(fields);
  }
  if (valid) { state->owner_uid = (uid_t)uid; state->owner_pid = (pid_t)pid; }
  g_strfreev(owner); g_strfreev(interface); g_strfreev(lines); g_free(contents);
  if (!valid) { stored_activation_clear(state); g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "invalid activation state"); return FALSE; }
  return TRUE;
}
static gboolean remove_state_file(const char *token, GError **error) {
  gchar *path = g_build_filename(STATE_DIR, token, NULL);
  gboolean ok = unlink(path) == 0 || errno == ENOENT;
  int saved = errno;
  g_free(path);
  if (!ok) g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(saved), "remove activation state: %s", g_strerror(saved));
  return ok;
}
static gboolean valid_legacy_token(const char *token) {
  if (!token || strlen(token) != 16) return FALSE;
  for (const char *p = token; *p; p++) if (!g_ascii_isxdigit(*p)) return FALSE;
  return TRUE;
}
static gboolean remove_orphaned_legacy_state(const char *token, GError **error) {
  gchar *path = g_build_filename(STATE_DIR, token, NULL);
  int fd = open(path, O_RDONLY | O_NOFOLLOW); struct stat st;
  gboolean safe = fd >= 0 && fstat(fd, &st) == 0 && st.st_uid == STATE_OWNER_UID && (st.st_mode & 077) == 0 && S_ISREG(st.st_mode) && st.st_size > 0 && st.st_size < IFNAMSIZ;
  gchar buffer[IFNAMSIZ] = { 0 };
  if (safe) safe = read(fd, buffer, sizeof(buffer) - 1) == st.st_size;
  if (fd >= 0) close(fd);
  gchar *name = g_strstrip(buffer);
  if (g_str_has_suffix(name, "\\n")) name[strlen(name) - 2] = 0;
  safe = safe && valid_name(name) && (g_str_has_prefix(name, "fwg") || g_str_has_prefix(name, "epair"));
  gboolean ok = TRUE;
  if (safe && if_nametoindex(name) == 0 && unlink(path) != 0 && errno != ENOENT) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "remove stale legacy activation state: %s", g_strerror(errno)); ok = FALSE;
  }
  g_free(path); return ok;
}

static gboolean remove_endpoint_routes(GPtrArray *routes, GError **error) {
  gboolean ok = TRUE;
  for (guint index = routes ? routes->len : 0; index > 0; index--) {
    gchar **fields = g_strsplit(g_ptr_array_index(routes, index - 1), "\t", 3);
    const char *route_family = g_str_equal(fields[0], "4") ? "-inet" : "-inet6";
    char *argv[] = { (char *)ROUTE, "-n", "delete", (char *)route_family, "-host", fields[1], fields[2], NULL };
    GError *local = NULL;
    if (!run(argv, &local)) {
      if (local && strstr(local->message, "not in table")) g_clear_error(&local);
      else { if (!*error) g_propagate_error(error, local); else g_clear_error(&local); ok = FALSE; }
    }
    g_strfreev(fields);
  }
  return ok;
}
static void remember_cleanup_result(gboolean result, GError *local, gboolean *ok, GError **error) {
  if (result) return;
  *ok = FALSE;
  if (!*error) g_propagate_error(error, local); else g_clear_error(&local);
}
static gboolean cleanup_activation(Activation *activation, gboolean wireguard, GError **error) {
  gboolean resources_ok = TRUE;
  GError *local = NULL;
  if (activation->interface_name) {
    char *destroy[] = { (char *)IFCONFIG, activation->interface_name, "destroy", NULL };
    gboolean result = run(destroy, &local);
    remember_cleanup_result(result, local, &resources_ok, error); local = NULL;
  }
  if (activation->rename_interface_name) {
    char *destroy[] = { (char *)IFCONFIG, activation->rename_interface_name, "destroy", NULL };
    gboolean result = run(destroy, &local);
    remember_cleanup_result(result, local, &resources_ok, error); local = NULL;
  }
  gboolean routes_removed = remove_endpoint_routes(activation->routes, &local);
  remember_cleanup_result(routes_removed, local, &resources_ok, error); local = NULL;
  if (wireguard && activation->dns_installed) {
    gboolean dns_removed = remove_dns(activation->interface_name, &local);
    remember_cleanup_result(dns_removed, local, &resources_ok, error); local = NULL;
  }
  if (resources_ok) {
    gboolean state_removed = remove_state_file(activation->token, &local);
    remember_cleanup_result(state_removed, local, &resources_ok, error);
  }
  return resources_ok;
}
static gboolean activate(const char *type, GVariant *settings, GError **error) {
  if (!g_str_equal(type, "wireguard") && !g_str_equal(type, "dummy")) { g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED, "unsupported connection type"); return FALSE; }
  Activation activation = { .token = new_token(), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .peer_routes = TRUE, .owner_uid = getuid(), .owner_pid = getppid() };
  gboolean ok;
  if (g_str_equal(type, "wireguard")) {
    activation.interface_name = new_wireguard_clone_name();
    ok = save_state(&activation, error);
    if (ok) ok = create_wireguard_interface(&activation, error);
  } else {
    activation.interface_name = new_epair_name();
    ok = save_state(&activation, error);
    if (ok) ok = create_dummy_interface(&activation, error);
  }
  if (ok && g_str_equal(type, "wireguard")) ok = configure_wireguard(&activation, settings, error);
  if (ok && g_str_equal(type, "dummy")) ok = configure_addresses(settings, "ipv6", AF_INET6, activation.interface_name, error) && configure_addresses(settings, "ipv4", AF_INET, activation.interface_name, error);
  if (ok) { char *up[] = { (char *)IFCONFIG, activation.interface_name, "up", NULL }; ok = run(up, error); }
  if (ok && g_str_equal(type, "wireguard")) ok = configure_full_tunnel(&activation, error);
  if (ok && g_str_equal(type, "wireguard")) ok = configure_dns(&activation, settings, error);
  if (!ok && activation.interface_name) {
    GError *cleanup_error = NULL;
    cleanup_activation(&activation, g_str_equal(type, "wireguard"), &cleanup_error);
    g_clear_error(&cleanup_error);
  }
  if (ok) g_print("%s %s %u %u\n", activation.token, activation.interface_name, activation.default_ipv4 ? 1u : 0u, activation.default_ipv6 ? 1u : 0u);
  g_ptr_array_unref(activation.routes); g_ptr_array_unref(activation.endpoints); g_free(activation.token); g_free(activation.interface_name); g_free(activation.rename_interface_name); return ok;
}
static gboolean cleanup_loaded_state(const char *token, StoredActivation *state, GError **error) {
  gboolean resources_ok = TRUE;
  GError *local = NULL;
  if (if_nametoindex(state->interface_name) != 0) {
    char *destroy[] = { (char *)IFCONFIG, state->interface_name, "destroy", NULL };
    gboolean result = run(destroy, &local);
    remember_cleanup_result(result, local, &resources_ok, error); local = NULL;
  }
  if (state->rename_interface_name && if_nametoindex(state->rename_interface_name) != 0) {
    char *destroy[] = { (char *)IFCONFIG, state->rename_interface_name, "destroy", NULL };
    gboolean result = run(destroy, &local);
    remember_cleanup_result(result, local, &resources_ok, error); local = NULL;
  }
  gboolean routes_removed = remove_endpoint_routes(state->routes, &local);
  remember_cleanup_result(routes_removed, local, &resources_ok, error); local = NULL;
  if (g_str_has_prefix(state->interface_name, "fwg")) {
    gboolean dns_removed = remove_dns(state->interface_name, &local);
    remember_cleanup_result(dns_removed, local, &resources_ok, error); local = NULL;
  }
  if (resources_ok) {
    gboolean state_removed = remove_state_file(token, &local);
    remember_cleanup_result(state_removed, local, &resources_ok, error);
  }
  return resources_ok;
}
static gboolean deactivate(const char *token, GError **error) {
  StoredActivation state = { 0 };
  if (!load_state(token, &state, error)) return FALSE;
  if (state.owner_uid != getuid() || state.owner_pid != getppid()) {
    stored_activation_clear(&state);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED, "activation handle is not owned by this compatibility service");
    return FALSE;
  }
  gboolean ok = cleanup_loaded_state(token, &state, error);
  stored_activation_clear(&state);
  return ok;
}
static gboolean recover_orphaned_dns(GError **error) {
  gchar *names = NULL;
  char *list[] = { (char *)RESOLVCONF, "-i", NULL };
  if (!run_capture(list, &names, error)) return FALSE;
  gboolean ok = TRUE;
  gchar **entries = g_strsplit_set(names, " \t\r\n", -1);
  for (guint index = 0; entries[index]; index++) {
    const char *entry = entries[index];
    const char *suffix = ".wireguard";
    if (!g_str_has_suffix(entry, suffix)) continue;
    gsize length = strlen(entry) - strlen(suffix);
    gchar *interface_name = g_strndup(entry, length);
    if (valid_name(interface_name) && g_str_has_prefix(interface_name, "fwg") && if_nametoindex(interface_name) == 0) {
      GError *local = NULL;
      if (!remove_dns(interface_name, &local)) {
        if (!*error) g_propagate_error(error, local); else g_clear_error(&local);
        ok = FALSE;
      }
    }
    g_free(interface_name);
  }
  g_strfreev(entries);
  g_free(names);
  return ok;
}
static gboolean recover_stale_activations(GError **error) {
  DIR *directory = opendir(STATE_DIR);
  if (!directory) { g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "open helper state directory: %s", g_strerror(errno)); return FALSE; }
  gboolean ok = TRUE;
  struct dirent *entry;
  while ((entry = readdir(directory))) {
    if (valid_legacy_token(entry->d_name)) {
      GError *legacy_error = NULL;
      if (!remove_orphaned_legacy_state(entry->d_name, &legacy_error)) { if (!*error) g_propagate_error(error, legacy_error); else g_clear_error(&legacy_error); ok = FALSE; }
      continue;
    }
    if (!valid_token(entry->d_name)) continue;
    StoredActivation state = { 0 };
    GError *local = NULL;
    if (!load_state(entry->d_name, &state, &local)) { g_clear_error(&local); continue; }
    if (state.owner_uid == getuid() && !pid_alive(state.owner_pid)) {
      if (!cleanup_loaded_state(entry->d_name, &state, &local)) {
        if (!*error) g_propagate_error(error, local); else g_clear_error(&local);
        ok = FALSE;
      }
    }
    stored_activation_clear(&state);
  }
  closedir(directory);
  GError *dns_error = NULL;
  if (!recover_orphaned_dns(&dns_error)) {
    if (!*error) g_propagate_error(error, dns_error); else g_clear_error(&dns_error);
    ok = FALSE;
  }
  return ok;
}
int main(int argc, char **argv) {
  GError *error = NULL;
  if (!sanitize_environment(&error) || !caller_authorized(&error) || !ensure_state_layout(&error)) {
    die(error ? error->message : "helper setup failed"); g_clear_error(&error); return 1;
  }
  int lock_fd = acquire_state_lock(&error);
  if (lock_fd < 0) { die(error->message); g_clear_error(&error); return 1; }
  if (!recover_stale_activations(&error)) {
    die(error ? error->message : "stale activation recovery failed"); g_clear_error(&error); close(lock_fd); return 1;
  }
  if ((argc != 3 || (!g_str_equal(argv[1], "activate") && !g_str_equal(argv[1], "deactivate"))) || (g_str_equal(argv[1], "activate") && !g_str_equal(argv[2], "wireguard") && !g_str_equal(argv[2], "dummy"))) { die("usage: activate TYPE | deactivate TOKEN"); close(lock_fd); return 64; }
  gboolean ok;
  if (g_str_equal(argv[1], "deactivate")) ok = deactivate(argv[2], &error);
  else {
    GByteArray *bytes = g_byte_array_new(); guint8 buffer[8192]; ssize_t count;
    while ((count = read(STDIN_FILENO, buffer, sizeof(buffer))) > 0 && bytes->len <= MAX_SETTINGS_INPUT) g_byte_array_append(bytes, buffer, count);
    if (count < 0 || bytes->len > MAX_SETTINGS_INPUT) { g_byte_array_unref(bytes); die("invalid settings input"); close(lock_fd); return 1; }
    GVariant *settings = g_variant_new_from_data(G_VARIANT_TYPE("a{sa{sv}}"), bytes->data, bytes->len, FALSE, (GDestroyNotify)g_byte_array_unref, bytes);
    if (!g_variant_is_normal_form(settings)) { g_variant_unref(settings); die("settings are not a normal variant"); close(lock_fd); return 1; }
    ok = activate(argv[2], settings, &error); g_variant_unref(settings);
  }
  close(lock_fd);
  if (!ok) { die(error ? error->message : "operation failed"); g_clear_error(&error); return 1; }
  return 0;
}
