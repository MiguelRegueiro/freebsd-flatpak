#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define DRM_PRELOAD "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so"

struct preload_state {
    char *old_value;
    int was_set;
    int changed;
};

static int is_zygote(char *const argv[]) {
    int type = 0;
    int no_sandbox = 0;

    if (argv == NULL) {
        return 0;
    }

    for (int i = 1; argv[i] != NULL; i++) {
        if (strcmp(argv[i], "--type=zygote") == 0) {
            type = 1;
        }
        if (strcmp(argv[i], "--no-zygote-sandbox") == 0) {
            no_sandbox = 1;
        }
    }

    return type && no_sandbox;
}

static void discard_preload_state(struct preload_state *state) {
    free(state->old_value);
    state->old_value = NULL;
    state->changed = 0;
}

static void restore_preload(struct preload_state *state) {
    if (!state->changed) {
        return;
    }

    if (state->was_set) {
        (void)setenv("LD_PRELOAD", state->old_value, 1);
    } else {
        (void)unsetenv("LD_PRELOAD");
    }

    discard_preload_state(state);
}

static int inject_preload(char *const argv[], struct preload_state *state) {
    const char *old;
    char *new_value;

    memset(state, 0, sizeof(*state));
    if (!is_zygote(argv)) {
        return 0;
    }

    old = getenv("LD_PRELOAD");
    state->was_set = old != NULL;
    if (state->was_set) {
        state->old_value = strdup(old);
        if (state->old_value == NULL) {
            return -1;
        }
    }

    if (old != NULL && *old != '\0') {
        size_t drm_len = strlen(DRM_PRELOAD);
        size_t old_len = strlen(old);

        if (old_len > SIZE_MAX - drm_len - 2) {
            discard_preload_state(state);
            errno = ENOMEM;
            return -1;
        }

        new_value = malloc(drm_len + 1 + old_len + 1);
        if (new_value == NULL) {
            discard_preload_state(state);
            return -1;
        }

        memcpy(new_value, DRM_PRELOAD, drm_len);
        new_value[drm_len] = ':';
        memcpy(new_value + drm_len + 1, old, old_len + 1);
    } else {
        new_value = strdup(DRM_PRELOAD);
        if (new_value == NULL) {
            discard_preload_state(state);
            return -1;
        }
    }

    if (setenv("LD_PRELOAD", new_value, 1) != 0) {
        int saved_errno = errno;

        free(new_value);
        discard_preload_state(state);
        errno = saved_errno;
        return -1;
    }

    free(new_value);
    state->changed = 1;
    return 1;
}

int execvp(const char *file, char *const argv[]) {
    static int (*real_execvp)(const char *, char *const[]);
    struct preload_state state;
    int result;
    int saved_errno;

    if (real_execvp == NULL) {
        real_execvp = dlsym(RTLD_NEXT, "execvp");
        if (real_execvp == NULL) {
            errno = ENOSYS;
            return -1;
        }
    }

    if (inject_preload(argv, &state) < 0) {
        return -1;
    }

    result = real_execvp(file, argv);
    saved_errno = errno;
    restore_preload(&state);
    errno = saved_errno;
    return result;
}

#ifdef CHROMIUM_ZYGOTE_DRM_PRELOAD_TEST
int chromium_zygote_should_inject(char *const argv[]) {
    return is_zygote(argv);
}

int chromium_zygote_inject_for_test(char *const argv[]) {
    struct preload_state state;
    int result = inject_preload(argv, &state);

    discard_preload_state(&state);
    return result;
}
#endif
