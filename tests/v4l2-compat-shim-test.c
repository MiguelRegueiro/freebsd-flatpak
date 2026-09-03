#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct _GUdevClient GUdevClient;
typedef struct _GUdevDevice GUdevDevice;
typedef struct _GList {
  void *data;
  struct _GList *next;
  struct _GList *prev;
} GList;

int ioctl(int fd, unsigned long request, void *arg);
int open(const char *path, int flags, ...);
int open64(const char *path, int flags, ...);
GList *g_udev_client_query_by_subsystem(GUdevClient *client,
                                        const char *subsystem);
const char *g_udev_device_get_device_file(GUdevDevice *device);
const char *g_udev_device_get_sysfs_path(GUdevDevice *device);
const char *g_udev_device_get_property(GUdevDevice *device, const char *key);
int g_udev_device_get_property_as_int(GUdevDevice *device, const char *key);

static int stub_result;
static int stub_errno;
static int seen_fd;
static unsigned long seen_request;
static void *seen_arg;
static const char *seen_path;
static int seen_open_flags;
static mode_t seen_open_mode;
static GList *query_result;
static const char *seen_subsystem;
static uint64_t video_nodes;

int v4l2_compat_shim_test_real_ioctl(int fd, unsigned long request, void *arg) {
  seen_fd = fd;
  seen_request = request;
  seen_arg = arg;
  errno = stub_errno;
  return stub_result;
}

int v4l2_compat_shim_test_real_open(const char *path, int flags, mode_t mode) {
  seen_path = path;
  seen_open_flags = flags;
  seen_open_mode = mode;
  return 42;
}

GList *v4l2_compat_shim_test_real_query(GUdevClient *client,
                                        const char *subsystem) {
  (void)client;
  seen_subsystem = subsystem;
  return query_result;
}

int v4l2_compat_shim_test_video_exists(unsigned int index) {
  return (video_nodes & (UINT64_C(1) << index)) != 0;
}

static void configure_stub(int result, int error) {
  stub_result = result;
  stub_errno = error;
  seen_fd = -1;
  seen_request = 0;
  seen_arg = NULL;
}

static void assert_forwarded(int fd, unsigned long request, void *arg) {
  assert(seen_fd == fd);
  assert(seen_request == request);
  assert(seen_arg == arg);
}

static int gstreamer_auto_uses_mmap(int export_probe_errno) {
  struct v4l2_exportbuffer export = {
      .type = V4L2_BUF_TYPE_VIDEO_CAPTURE,
      .index = (unsigned int)-1,
      .plane = (unsigned int)-1,
  };

  configure_stub(-1, export_probe_errno);
  assert(ioctl(7, VIDIOC_EXPBUF, &export) == -1);
  return errno == ENOTTY;
}

static void camera_opens_cannot_acquire_a_controlling_terminal(void) {
  assert(open64("/dev/video0", O_RDWR | O_NONBLOCK) == 42);
  assert(strcmp(seen_path, "/dev/video0") == 0);
  assert(seen_open_flags == (O_RDWR | O_NONBLOCK | O_NOCTTY));
  assert(seen_open_mode == 0);

  assert(open("/dev/video12", O_RDONLY) == 42);
  assert(seen_open_flags == (O_RDONLY | O_NOCTTY));
}

static void unrelated_opens_are_untouched(void) {
  assert(open64("/dev/dri/card0", O_RDWR) == 42);
  assert(seen_open_flags == O_RDWR);
  assert(seen_open_mode == 0);

  assert(open("/tmp/video", O_CREAT | O_WRONLY, 0640) == 42);
  assert(seen_open_flags == (O_CREAT | O_WRONLY));
  assert(seen_open_mode == 0640);
}

static void missing_video4linux_sysfs_falls_back_to_device_nodes(void) {
  GList *devices;

  query_result = NULL;
  video_nodes = (UINT64_C(1) << 0) | (UINT64_C(1) << 2);
  devices = g_udev_client_query_by_subsystem(NULL, "video4linux");
  assert(strcmp(seen_subsystem, "video4linux") == 0);
  assert(devices != NULL && devices->next != NULL);
  assert(devices->next->next == NULL);
  assert(strcmp(g_udev_device_get_device_file(devices->data), "/dev/video0") ==
         0);
  assert(strcmp(g_udev_device_get_sysfs_path(devices->next->data),
                "/sys/class/video4linux/video2") == 0);
  assert(strcmp(g_udev_device_get_property(devices->data, "SUBSYSTEM"),
                "video4linux") == 0);
  assert(g_udev_device_get_property_as_int(devices->data, "ID_V4L_VERSION") ==
         2);
  assert(g_udev_device_get_property(devices->data, "ID_MODEL") == NULL);
  free(devices->next);
  free(devices);
}

static void native_and_unrelated_discovery_are_untouched(void) {
  GList native = {.data = (void *)(uintptr_t)99};

  video_nodes = UINT64_MAX;
  query_result = &native;
  assert(g_udev_client_query_by_subsystem(NULL, "video4linux") == &native);

  query_result = NULL;
  assert(g_udev_client_query_by_subsystem(NULL, "sound") == NULL);
  assert(strcmp(seen_subsystem, "sound") == 0);
}

static void unsupported_export_selects_mmap(void) {
  assert(gstreamer_auto_uses_mmap(EINVAL));
}

static void successful_export_is_untouched(void) {
  struct v4l2_exportbuffer export = {0};
  configure_stub(0, EINVAL);

  assert(ioctl(8, VIDIOC_EXPBUF, &export) == 0);
  assert(errno == EINVAL);
  assert_forwarded(8, VIDIOC_EXPBUF, &export);
}

static void other_export_errors_are_untouched(void) {
  struct v4l2_exportbuffer export = {0};
  configure_stub(-1, EBADF);

  assert(ioctl(9, VIDIOC_EXPBUF, &export) == -1);
  assert(errno == EBADF);
  assert_forwarded(9, VIDIOC_EXPBUF, &export);
}

static void unrelated_camera_ioctls_are_untouched(void) {
  struct v4l2_requestbuffers request = {0};
  configure_stub(-1, EINVAL);

  assert(ioctl(10, VIDIOC_REQBUFS, &request) == -1);
  assert(errno == EINVAL);
  assert_forwarded(10, VIDIOC_REQBUFS, &request);
}

static void unrelated_ioctls_are_untouched(void) {
  int argument = 0;
  configure_stub(-1, EINVAL);

  assert(ioctl(11, 0x1234UL, &argument) == -1);
  assert(errno == EINVAL);
  assert_forwarded(11, 0x1234UL, &argument);
}

int main(void) {
  camera_opens_cannot_acquire_a_controlling_terminal();
  unrelated_opens_are_untouched();
  missing_video4linux_sysfs_falls_back_to_device_nodes();
  native_and_unrelated_discovery_are_untouched();
  unsupported_export_selects_mmap();
  successful_export_is_untouched();
  other_export_errors_are_untouched();
  unrelated_camera_ioctls_are_untouched();
  unrelated_ioctls_are_untouched();
  puts("V4L2 compatibility shim tests passed");
  return 0;
}
