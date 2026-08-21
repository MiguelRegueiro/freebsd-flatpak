#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define DRM_PRELOAD "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so"

int chromium_zygote_should_inject(char *const argv[]);
int chromium_zygote_inject_for_test(char *const argv[]);

static void test_matches_only_unsandboxed_zygote(void) {
    char *zygote[] = {"Chromium", "--type=zygote", "--no-zygote-sandbox", NULL};
    char *zygote_reordered[] = {
        "electron", "--no-zygote-sandbox", "--type=zygote", "--other-flag", NULL};
    char *gpu[] = {"Chromium", "--type=gpu-process", NULL};
    char *zygote_sandboxed[] = {"Chromium", "--type=zygote", NULL};
    char *other[] = {"Chromium", "--type=renderer", "--no-zygote-sandbox", NULL};

    assert(chromium_zygote_should_inject(zygote));
    assert(chromium_zygote_should_inject(zygote_reordered));
    assert(!chromium_zygote_should_inject(gpu));
    assert(!chromium_zygote_should_inject(zygote_sandboxed));
    assert(!chromium_zygote_should_inject(other));
}

static void test_injects_only_drm_preload(void) {
    char *zygote[] = {"Chromium", "--type=zygote", "--no-zygote-sandbox", NULL};

    assert(unsetenv("LD_PRELOAD") == 0);
    assert(chromium_zygote_inject_for_test(zygote) == 1);
    assert(strcmp(getenv("LD_PRELOAD"), DRM_PRELOAD) == 0);
    assert(strstr(getenv("LD_PRELOAD"), "wayland") == NULL);
    assert(unsetenv("LD_PRELOAD") == 0);
}

static void test_non_match_preserves_preload(void) {
    char *gpu[] = {"electron", "--type=gpu-process", NULL};

    assert(setenv("LD_PRELOAD", "/app/existing.so", 1) == 0);
    assert(chromium_zygote_inject_for_test(gpu) == 0);
    assert(strcmp(getenv("LD_PRELOAD"), "/app/existing.so") == 0);
    assert(unsetenv("LD_PRELOAD") == 0);
}

static void test_prepends_to_existing_preload(void) {
    char *zygote[] = {"electron", "--type=zygote", "--no-zygote-sandbox", NULL};
    const char *expected = DRM_PRELOAD ":/app/existing-a.so:/app/existing-b.so";

    assert(setenv("LD_PRELOAD", "/app/existing-a.so:/app/existing-b.so", 1) == 0);
    assert(chromium_zygote_inject_for_test(zygote) == 1);
    assert(strcmp(getenv("LD_PRELOAD"), expected) == 0);
    assert(strstr(getenv("LD_PRELOAD"), "wayland") == NULL);
    assert(unsetenv("LD_PRELOAD") == 0);
}

static void test_failed_exec_restores_existing_preload(void) {
    char *zygote[] = {"Chromium", "--type=zygote", "--no-zygote-sandbox", NULL};

    assert(setenv("LD_PRELOAD", "/app/original.so", 1) == 0);
    errno = 0;
    assert(execvp("/definitely/not/a/chromium-binary", zygote) == -1);
    assert(errno == ENOENT);
    assert(strcmp(getenv("LD_PRELOAD"), "/app/original.so") == 0);
    assert(unsetenv("LD_PRELOAD") == 0);
}

static void test_failed_exec_restores_absent_preload(void) {
    char *zygote[] = {"electron", "--type=zygote", "--no-zygote-sandbox", NULL};

    assert(unsetenv("LD_PRELOAD") == 0);
    errno = 0;
    assert(execvp("/definitely/not/an/electron-binary", zygote) == -1);
    assert(errno == ENOENT);
    assert(getenv("LD_PRELOAD") == NULL);
}

int main(void) {
    test_matches_only_unsandboxed_zygote();
    test_injects_only_drm_preload();
    test_non_match_preserves_preload();
    test_prepends_to_existing_preload();
    test_failed_exec_restores_existing_preload();
    test_failed_exec_restores_absent_preload();
    puts("Chromium zygote preload tests passed");
    return 0;
}
