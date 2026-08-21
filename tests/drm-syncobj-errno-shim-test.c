#include <assert.h>
#include <errno.h>
#include <stdio.h>

#include <drm/drm.h>

int ioctl(int fd, unsigned long request, void *arg);

static int stub_result;
static int stub_errno;
static int seen_fd;
static unsigned long seen_request;
static void *seen_arg;

int drm_syncobj_errno_shim_test_real_ioctl(int fd, unsigned long request,
                                            void *arg)
{
    seen_fd = fd;
    seen_request = request;
    seen_arg = arg;
    errno = stub_errno;
    return stub_result;
}

static void configure_stub(int result, int error)
{
    stub_result = result;
    stub_errno = error;
    seen_fd = -1;
    seen_request = 0;
    seen_arg = NULL;
}

static void assert_forwarded(int fd, unsigned long request, void *arg)
{
    assert(seen_fd == fd);
    assert(seen_request == request);
    assert(seen_arg == arg);
}

static void unrelated_ioctl_is_untouched(void)
{
    int argument = 1;
    const unsigned long unrelated_request = 0x1234UL;
    configure_stub(-1, ETIMEDOUT);

    assert(ioctl(7, unrelated_request, &argument) == -1);
    assert(errno == ETIMEDOUT);
    assert_forwarded(7, unrelated_request, &argument);
}

static void successful_syncobj_wait_is_untouched(void)
{
    struct drm_syncobj_wait wait = {0};
    configure_stub(0, EAGAIN);

    assert(ioctl(8, DRM_IOCTL_SYNCOBJ_WAIT, &wait) == 0);
    assert(errno == EAGAIN);
    assert_forwarded(8, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
}

static void other_syncobj_wait_errno_is_untouched(void)
{
    struct drm_syncobj_wait wait = {0};
    configure_stub(-1, EINVAL);

    assert(ioctl(9, DRM_IOCTL_SYNCOBJ_WAIT, &wait) == -1);
    assert(errno == EINVAL);
    assert_forwarded(9, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
}

static void syncobj_wait_etimedout_becomes_etime(void)
{
    struct drm_syncobj_wait wait = {0};
    configure_stub(-1, ETIMEDOUT);

    assert(ioctl(10, DRM_IOCTL_SYNCOBJ_WAIT, &wait) == -1);
    assert(errno == ETIME);
    assert_forwarded(10, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
}

int main(void)
{
    unrelated_ioctl_is_untouched();
    successful_syncobj_wait_is_untouched();
    other_syncobj_wait_errno_is_untouched();
    syncobj_wait_etimedout_becomes_etime();
    puts("drm-syncobj-errno-shim tests passed");
    return 0;
}
