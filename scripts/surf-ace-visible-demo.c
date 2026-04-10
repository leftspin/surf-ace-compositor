#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"

struct app_state {
    struct wl_display *display;
    struct wl_registry *registry;
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    struct xdg_wm_base *wm_base;

    struct wl_surface *surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *xdg_toplevel;
    struct wl_callback *frame_cb;

    struct wl_buffer *buffer;
    void *pixels;
    int shm_fd;
    size_t shm_size;

    int width;
    int height;
    int pending_width;
    int pending_height;
    bool running;
    uint32_t frame_no;
};

static struct app_state *g_app;

static int create_tmp_shm(size_t size) {
    char template[] = "/dev/shm/surf-ace-demo-XXXXXX";
    int fd = mkstemp(template);
    if (fd < 0) return -1;
    unlink(template);
    if (ftruncate(fd, (off_t)size) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static void destroy_buffer(struct app_state *app) {
    if (app->frame_cb) {
        wl_callback_destroy(app->frame_cb);
        app->frame_cb = NULL;
    }
    if (app->buffer) {
        wl_buffer_destroy(app->buffer);
        app->buffer = NULL;
    }
    if (app->pixels) {
        munmap(app->pixels, app->shm_size);
        app->pixels = NULL;
    }
    if (app->shm_fd >= 0) {
        close(app->shm_fd);
        app->shm_fd = -1;
    }
    app->shm_size = 0;
}

static const struct wl_callback_listener frame_listener;

static int ensure_buffer(struct app_state *app) {
    if (app->width <= 0 || app->height <= 0) return -1;
    size_t stride = (size_t)app->width * 4;
    size_t need = stride * (size_t)app->height;

    if (app->buffer && app->pixels && app->shm_size == need) {
        return 0;
    }

    destroy_buffer(app);

    app->shm_fd = create_tmp_shm(need);
    if (app->shm_fd < 0) {
        fprintf(stderr, "failed to create shm file: %s\n", strerror(errno));
        return -1;
    }

    app->pixels = mmap(NULL, need, PROT_READ | PROT_WRITE, MAP_SHARED, app->shm_fd, 0);
    if (app->pixels == MAP_FAILED) {
        app->pixels = NULL;
        fprintf(stderr, "mmap failed: %s\n", strerror(errno));
        destroy_buffer(app);
        return -1;
    }
    app->shm_size = need;

    struct wl_shm_pool *pool = wl_shm_create_pool(app->shm, app->shm_fd, (int)need);
    app->buffer = wl_shm_pool_create_buffer(
        pool,
        0,
        app->width,
        app->height,
        (int)stride,
        WL_SHM_FORMAT_XRGB8888
    );
    wl_shm_pool_destroy(pool);
    if (!app->buffer) {
        fprintf(stderr, "failed to create wl_buffer\n");
        destroy_buffer(app);
        return -1;
    }

    return 0;
}

static void paint_frame(struct app_state *app) {
    if (ensure_buffer(app) != 0) return;

    uint32_t *px = (uint32_t *)app->pixels;
    int w = app->width;
    int h = app->height;
    uint32_t t = app->frame_no;

    for (int y = 0; y < h; y++) {
        for (int x = 0; x < w; x++) {
            uint8_t r = (uint8_t)((x + t * 3) & 0xFF);
            uint8_t g = (uint8_t)((y * 2 + t * 5) & 0xFF);
            uint8_t b = (uint8_t)(((x ^ y) + t * 7) & 0xFF);
            if ((x / 80 + y / 80) % 2 == 0) {
                r = (uint8_t)(255 - r / 2);
                g = (uint8_t)(255 - g / 2);
                b = (uint8_t)(255 - b / 2);
            }
            px[(size_t)y * (size_t)w + (size_t)x] = (uint32_t)(r << 16 | g << 8 | b);
        }
    }

    wl_surface_attach(app->surface, app->buffer, 0, 0);
    wl_surface_damage_buffer(app->surface, 0, 0, app->width, app->height);
    app->frame_cb = wl_surface_frame(app->surface);
    wl_callback_add_listener(app->frame_cb, &frame_listener, app);
    wl_surface_commit(app->surface);
    app->frame_no++;
}

static void frame_done(void *data, struct wl_callback *cb, uint32_t time) {
    (void)time;
    struct app_state *app = data;
    if (cb) wl_callback_destroy(cb);
    app->frame_cb = NULL;
    if (!app->running) return;
    paint_frame(app);
}

static const struct wl_callback_listener frame_listener = {
    .done = frame_done,
};

static void xdg_wm_base_ping(void *data, struct xdg_wm_base *wm_base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(wm_base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {
    .ping = xdg_wm_base_ping,
};

static void xdg_surface_configure(void *data, struct xdg_surface *surface, uint32_t serial) {
    struct app_state *app = data;
    xdg_surface_ack_configure(surface, serial);

    if (app->pending_width > 0) app->width = app->pending_width;
    if (app->pending_height > 0) app->height = app->pending_height;
    if (app->width <= 0) app->width = 1280;
    if (app->height <= 0) app->height = 720;

    if (!app->frame_cb) {
        paint_frame(app);
    }
}

static const struct xdg_surface_listener xdg_surface_listener = {
    .configure = xdg_surface_configure,
};

static void toplevel_configure(
    void *data,
    struct xdg_toplevel *toplevel,
    int32_t width,
    int32_t height,
    struct wl_array *states
) {
    (void)toplevel;
    (void)states;
    struct app_state *app = data;
    if (width > 0) app->pending_width = width;
    if (height > 0) app->pending_height = height;
}

static void toplevel_close(void *data, struct xdg_toplevel *toplevel) {
    (void)toplevel;
    struct app_state *app = data;
    app->running = false;
}

static const struct xdg_toplevel_listener toplevel_listener = {
    .configure = toplevel_configure,
    .close = toplevel_close,
    .configure_bounds = NULL,
    .wm_capabilities = NULL,
};

static void registry_global(
    void *data,
    struct wl_registry *registry,
    uint32_t name,
    const char *interface,
    uint32_t version
) {
    struct app_state *app = data;
    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        app->compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    } else if (strcmp(interface, wl_shm_interface.name) == 0) {
        app->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    } else if (strcmp(interface, xdg_wm_base_interface.name) == 0) {
        uint32_t bind_version = version < 2 ? version : 2;
        app->wm_base = wl_registry_bind(registry, name, &xdg_wm_base_interface, bind_version);
        xdg_wm_base_add_listener(app->wm_base, &wm_base_listener, app);
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

static void on_signal(int sig) {
    (void)sig;
    if (g_app) g_app->running = false;
}

int main(void) {
    struct app_state app = {0};
    app.shm_fd = -1;
    app.width = 1280;
    app.height = 720;
    app.running = true;
    g_app = &app;

    signal(SIGINT, on_signal);
    signal(SIGTERM, on_signal);

    app.display = wl_display_connect(NULL);
    if (!app.display) {
        fprintf(stderr, "failed to connect to wayland display\n");
        return 1;
    }

    app.registry = wl_display_get_registry(app.display);
    wl_registry_add_listener(app.registry, &registry_listener, &app);
    wl_display_roundtrip(app.display);
    wl_display_roundtrip(app.display);

    if (!app.compositor || !app.shm || !app.wm_base) {
        fprintf(
            stderr,
            "missing required globals compositor=%p shm=%p wm_base=%p\n",
            (void *)app.compositor,
            (void *)app.shm,
            (void *)app.wm_base
        );
        return 2;
    }

    app.surface = wl_compositor_create_surface(app.compositor);
    app.xdg_surface = xdg_wm_base_get_xdg_surface(app.wm_base, app.surface);
    xdg_surface_add_listener(app.xdg_surface, &xdg_surface_listener, &app);

    app.xdg_toplevel = xdg_surface_get_toplevel(app.xdg_surface);
    xdg_toplevel_add_listener(app.xdg_toplevel, &toplevel_listener, &app);
    xdg_toplevel_set_title(app.xdg_toplevel, "Surf Ace Visible Demo");
    xdg_toplevel_set_app_id(app.xdg_toplevel, "surf-ace-demo");
    xdg_toplevel_set_fullscreen(app.xdg_toplevel, NULL);

    wl_surface_commit(app.surface);
    wl_display_flush(app.display);

    while (app.running && wl_display_dispatch(app.display) != -1) {
        wl_display_flush(app.display);
    }

    destroy_buffer(&app);
    if (app.xdg_toplevel) xdg_toplevel_destroy(app.xdg_toplevel);
    if (app.xdg_surface) xdg_surface_destroy(app.xdg_surface);
    if (app.surface) wl_surface_destroy(app.surface);
    if (app.wm_base) xdg_wm_base_destroy(app.wm_base);
    if (app.shm) wl_shm_destroy(app.shm);
    if (app.compositor) wl_compositor_destroy(app.compositor);
    if (app.registry) wl_registry_destroy(app.registry);
    if (app.display) wl_display_disconnect(app.display);
    return 0;
}
