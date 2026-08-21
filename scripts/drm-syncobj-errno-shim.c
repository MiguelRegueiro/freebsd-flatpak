#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <string.h>

#include <drm/drm.h>

typedef int (*ioctl_fn)(int fd, unsigned long request, void *arg);

#ifdef DRM_SYNCOBJ_ERRNO_SHIM_TEST

extern int drm_syncobj_errno_shim_test_real_ioctl(int fd,
                                                   unsigned long request,
                                                   void *arg);

static int call_real_ioctl(int fd, unsigned long request, void *arg)
{
    return drm_syncobj_errno_shim_test_real_ioctl(fd, request, arg);
}

#else

static pthread_once_t resolve_ioctl_once = PTHREAD_ONCE_INIT;
static ioctl_fn real_ioctl;

static void resolve_ioctl(void)
{
    void *symbol = dlsym(RTLD_NEXT, "ioctl");
    memcpy(&real_ioctl, &symbol, sizeof(symbol));
}

static int call_real_ioctl(int fd, unsigned long request, void *arg)
{
    pthread_once(&resolve_ioctl_once, resolve_ioctl);
    if (real_ioctl == NULL) {
        errno = ENOSYS;
        return -1;
    }
    return real_ioctl(fd, request, arg);
}

#endif

int ioctl(int fd, unsigned long request, void *arg)
{
    int result = call_real_ioctl(fd, request, arg);

    /* Linux DRM specifies ETIME for an unsignaled syncobj wait. */
    if (request == DRM_IOCTL_SYNCOBJ_WAIT && result == -1 &&
        errno == ETIMEDOUT) {
        errno = ETIME;
    }

    return result;
}
