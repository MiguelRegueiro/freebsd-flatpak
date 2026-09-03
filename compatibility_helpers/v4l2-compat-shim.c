#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef int (*ioctl_fn)(int fd, unsigned long request, void *arg);
typedef int (*open_fn)(const char *path, int flags, mode_t mode);

/* Keep the GUdev/GObject ABI dependency optional: this shim is also loaded by
 * applications which never use GUdev. */
typedef struct _GUdevClient GUdevClient;
typedef struct _GUdevDevice GUdevDevice;
typedef struct _GList {
  void *data;
  struct _GList *next;
  struct _GList *prev;
} GList;
typedef GList *(*gudev_query_fn)(GUdevClient *client, const char *subsystem);
typedef const char *(*gudev_get_string_fn)(GUdevDevice *device);
typedef const char *(*gudev_get_property_fn)(GUdevDevice *device,
                                             const char *key);
typedef int (*gudev_get_property_int_fn)(GUdevDevice *device, const char *key);
typedef uintptr_t (*gudev_get_type_fn)(void);
typedef void *(*g_object_new_fn)(uintptr_t object_type,
                                 const char *first_property_name, ...);
typedef void (*g_weak_notify_fn)(void *data, void *object);
typedef void (*g_object_weak_ref_fn)(void *object, g_weak_notify_fn notify,
                                     void *data);

struct fake_video_device {
  GUdevDevice *object;
  unsigned int index;
  struct fake_video_device *next;
};

static pthread_mutex_t fake_devices_lock = PTHREAD_MUTEX_INITIALIZER;
static struct fake_video_device *fake_devices;

#ifdef V4L2_COMPAT_SHIM_TEST
extern int v4l2_compat_shim_test_real_ioctl(int fd, unsigned long request,
                                            void *arg);
extern int v4l2_compat_shim_test_real_open(const char *path, int flags,
                                           mode_t mode);
extern GList *v4l2_compat_shim_test_real_query(GUdevClient *client,
                                               const char *subsystem);
extern int v4l2_compat_shim_test_video_exists(unsigned int index);

static int call_real_ioctl(int fd, unsigned long request, void *arg) {
  return v4l2_compat_shim_test_real_ioctl(fd, request, arg);
}

static int call_real_open(const char *path, int flags, mode_t mode) {
  return v4l2_compat_shim_test_real_open(path, flags, mode);
}

static int call_real_open64(const char *path, int flags, mode_t mode) {
  return v4l2_compat_shim_test_real_open(path, flags, mode);
}

static GList *call_real_query(GUdevClient *client, const char *subsystem) {
  return v4l2_compat_shim_test_real_query(client, subsystem);
}

static GUdevDevice *new_empty_gudev_device(unsigned int index) {
  return (GUdevDevice *)(uintptr_t)(index + 1U);
}

static int video_device_exists(unsigned int index) {
  return v4l2_compat_shim_test_video_exists(index);
}
#else
static pthread_once_t resolve_libc_once = PTHREAD_ONCE_INIT;
static pthread_once_t resolve_gudev_once = PTHREAD_ONCE_INIT;
static ioctl_fn real_ioctl;
static open_fn real_open;
static open_fn real_open64;
static gudev_query_fn real_gudev_query;
static gudev_get_string_fn real_get_device_file;
static gudev_get_string_fn real_get_sysfs_path;
static gudev_get_property_fn real_get_property;
static gudev_get_property_int_fn real_get_property_as_int;
static gudev_get_type_fn real_get_device_type;
static g_object_new_fn real_object_new;
static g_object_weak_ref_fn real_object_weak_ref;

#define RESOLVE_FUNCTION(target, name)                                         \
  do {                                                                         \
    void *symbol = dlsym(RTLD_NEXT, name);                                     \
    memcpy(&(target), &symbol, sizeof(symbol));                                \
  } while (0)

static void resolve_libc(void) {
  RESOLVE_FUNCTION(real_ioctl, "ioctl");
  RESOLVE_FUNCTION(real_open, "open");
  RESOLVE_FUNCTION(real_open64, "open64");
}

static void resolve_gudev(void) {
  void *gudev = dlopen("libgudev-1.0.so.0", RTLD_LAZY | RTLD_LOCAL);
  void *gobject = dlopen("libgobject-2.0.so.0", RTLD_LAZY | RTLD_LOCAL);
  void *symbol;

  if (gudev != NULL) {
    symbol = dlsym(gudev, "g_udev_client_query_by_subsystem");
    memcpy(&real_gudev_query, &symbol, sizeof(symbol));
    symbol = dlsym(gudev, "g_udev_device_get_device_file");
    memcpy(&real_get_device_file, &symbol, sizeof(symbol));
    symbol = dlsym(gudev, "g_udev_device_get_sysfs_path");
    memcpy(&real_get_sysfs_path, &symbol, sizeof(symbol));
    symbol = dlsym(gudev, "g_udev_device_get_property");
    memcpy(&real_get_property, &symbol, sizeof(symbol));
    symbol = dlsym(gudev, "g_udev_device_get_property_as_int");
    memcpy(&real_get_property_as_int, &symbol, sizeof(symbol));
    symbol = dlsym(gudev, "g_udev_device_get_type");
    memcpy(&real_get_device_type, &symbol, sizeof(symbol));
  }
  if (gobject != NULL) {
    symbol = dlsym(gobject, "g_object_new");
    memcpy(&real_object_new, &symbol, sizeof(symbol));
    symbol = dlsym(gobject, "g_object_weak_ref");
    memcpy(&real_object_weak_ref, &symbol, sizeof(symbol));
  }
}

static int call_real_ioctl(int fd, unsigned long request, void *arg) {
  pthread_once(&resolve_libc_once, resolve_libc);
  if (real_ioctl == NULL) {
    errno = ENOSYS;
    return -1;
  }
  return real_ioctl(fd, request, arg);
}

static int call_real_open(const char *path, int flags, mode_t mode) {
  pthread_once(&resolve_libc_once, resolve_libc);
  if (real_open == NULL) {
    errno = ENOSYS;
    return -1;
  }
  return real_open(path, flags, mode);
}

static int call_real_open64(const char *path, int flags, mode_t mode) {
  pthread_once(&resolve_libc_once, resolve_libc);
  if (real_open64 == NULL) {
    errno = ENOSYS;
    return -1;
  }
  return real_open64(path, flags, mode);
}

static GList *call_real_query(GUdevClient *client, const char *subsystem) {
  pthread_once(&resolve_gudev_once, resolve_gudev);
  if (real_gudev_query == NULL)
    return NULL;
  return real_gudev_query(client, subsystem);
}

static GUdevDevice *new_empty_gudev_device(unsigned int index) {
  (void)index;
  pthread_once(&resolve_gudev_once, resolve_gudev);
  if (real_get_device_type == NULL || real_object_new == NULL ||
      real_object_weak_ref == NULL)
    return NULL;
  return real_object_new(real_get_device_type(), NULL);
}

static int video_device_exists(unsigned int index) {
  char path[32];

  if (snprintf(path, sizeof(path), "/dev/video%u", index) < 0)
    return 0;
  return access(path, F_OK) == 0;
}
#endif

static int is_video_device(const char *path) {
  static const char prefix[] = "/dev/video";
  const char *suffix;

  if (path == NULL || strncmp(path, prefix, sizeof(prefix) - 1) != 0)
    return 0;
  suffix = path + sizeof(prefix) - 1;
  if (*suffix == '\0')
    return 0;
  while (*suffix >= '0' && *suffix <= '9')
    suffix++;
  return *suffix == '\0';
}

static int fake_device_index(GUdevDevice *device, unsigned int *index) {
  struct fake_video_device *entry;
  int found = 0;

  pthread_mutex_lock(&fake_devices_lock);
  for (entry = fake_devices; entry != NULL; entry = entry->next) {
    if (entry->object == device) {
      *index = entry->index;
      found = 1;
      break;
    }
  }
  pthread_mutex_unlock(&fake_devices_lock);
  return found;
}

#ifndef V4L2_COMPAT_SHIM_TEST
static void remove_fake_device(void *data, void *object) {
  struct fake_video_device *entry = data;
  struct fake_video_device **cursor;

  (void)object;
  pthread_mutex_lock(&fake_devices_lock);
  for (cursor = &fake_devices; *cursor != NULL; cursor = &(*cursor)->next) {
    if (*cursor == entry) {
      *cursor = entry->next;
      break;
    }
  }
  pthread_mutex_unlock(&fake_devices_lock);
  free(entry);
}
#endif

static int register_fake_device(GUdevDevice *device, unsigned int index) {
  struct fake_video_device *entry = calloc(1, sizeof(*entry));

  if (entry == NULL)
    return 0;
  entry->object = device;
  entry->index = index;
  pthread_mutex_lock(&fake_devices_lock);
  entry->next = fake_devices;
  fake_devices = entry;
  pthread_mutex_unlock(&fake_devices_lock);
#ifndef V4L2_COMPAT_SHIM_TEST
  real_object_weak_ref(device, remove_fake_device, entry);
#endif
  return 1;
}

static GList *append_device(GList **tail, GUdevDevice *device) {
  GList *node = calloc(1, sizeof(*node));

  if (node == NULL)
    return NULL;
  node->data = device;
  node->prev = *tail;
  if (*tail != NULL)
    (*tail)->next = node;
  *tail = node;
  return node;
}

GList *g_udev_client_query_by_subsystem(GUdevClient *client,
                                        const char *subsystem) {
  GList *devices = call_real_query(client, subsystem);
  GList *head = NULL;
  GList *tail = NULL;
  unsigned int index;

  /* Native discovery wins. Only repair the Linuxulator environment where
   * /dev/videoN exists but sysfs has no video4linux class. */
  if (devices != NULL || subsystem == NULL ||
      strcmp(subsystem, "video4linux") != 0)
    return devices;

  for (index = 0; index < 64; index++) {
    GUdevDevice *device;
    GList *node;

    if (!video_device_exists(index))
      continue;
    device = new_empty_gudev_device(index);
    if (device == NULL || !register_fake_device(device, index))
      continue;
    node = append_device(&tail, device);
    if (node == NULL)
      continue;
    if (head == NULL)
      head = node;
  }
  return head;
}

const char *g_udev_device_get_device_file(GUdevDevice *device) {
  static _Thread_local char path[32];
  unsigned int index;

  if (fake_device_index(device, &index)) {
    snprintf(path, sizeof(path), "/dev/video%u", index);
    return path;
  }
#ifdef V4L2_COMPAT_SHIM_TEST
  return NULL;
#else
  pthread_once(&resolve_gudev_once, resolve_gudev);
  return real_get_device_file == NULL ? NULL : real_get_device_file(device);
#endif
}

const char *g_udev_device_get_sysfs_path(GUdevDevice *device) {
  static _Thread_local char path[64];
  unsigned int index;

  if (fake_device_index(device, &index)) {
    snprintf(path, sizeof(path), "/sys/class/video4linux/video%u", index);
    return path;
  }
#ifdef V4L2_COMPAT_SHIM_TEST
  return NULL;
#else
  pthread_once(&resolve_gudev_once, resolve_gudev);
  return real_get_sysfs_path == NULL ? NULL : real_get_sysfs_path(device);
#endif
}

const char *g_udev_device_get_property(GUdevDevice *device, const char *key) {
  unsigned int index;

  if (fake_device_index(device, &index)) {
    (void)index;
    if (key != NULL && strcmp(key, "ID_V4L_VERSION") == 0)
      return "2";
    if (key != NULL && strcmp(key, "SUBSYSTEM") == 0)
      return "video4linux";
    return NULL;
  }
#ifdef V4L2_COMPAT_SHIM_TEST
  return NULL;
#else
  pthread_once(&resolve_gudev_once, resolve_gudev);
  return real_get_property == NULL ? NULL : real_get_property(device, key);
#endif
}

int g_udev_device_get_property_as_int(GUdevDevice *device, const char *key) {
  unsigned int index;

  if (fake_device_index(device, &index)) {
    (void)index;
    return key != NULL && strcmp(key, "ID_V4L_VERSION") == 0 ? 2 : 0;
  }
#ifdef V4L2_COMPAT_SHIM_TEST
  return 0;
#else
  pthread_once(&resolve_gudev_once, resolve_gudev);
  return real_get_property_as_int == NULL
             ? 0
             : real_get_property_as_int(device, key);
#endif
}

int open(const char *path, int flags, ...) {
  mode_t mode = 0;

  if (flags & (O_CREAT | O_TMPFILE)) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, mode_t);
    va_end(arguments);
  }
  if (is_video_device(path))
    flags |= O_NOCTTY;
  return call_real_open(path, flags, mode);
}

int open64(const char *path, int flags, ...) {
  mode_t mode = 0;

  if (flags & (O_CREAT | O_TMPFILE)) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, mode_t);
    va_end(arguments);
  }
  if (is_video_device(path))
    flags |= O_NOCTTY;
  return call_real_open64(path, flags, mode);
}

int ioctl(int fd, unsigned long request, void *arg) {
  int result = call_real_ioctl(fd, request, arg);

  /* Linux V4L2 specifies ENOTTY for an unsupported buffer-export ioctl.
   * GStreamer relies on that distinction when choosing MMAP over DMABUF. */
  if (request == VIDIOC_EXPBUF && result == -1 && errno == EINVAL)
    errno = ENOTTY;

  return result;
}
