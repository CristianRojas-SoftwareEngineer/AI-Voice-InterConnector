/* unistd.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El fork qwen3-tts incluye unistd.h (no existe en MinGW) y llama:
 *   pread, setenv, unsetenv, posix_memalign, sysconf,
 *   posix_fadvise(fd, off, len, POSIX_FADV_DONTNEED).
 *
 * Plan 2.4: shims acotados.
 * - pread: _lseeki64 + _read.
 * - sysconf: env NUMBER_OF_PROCESSORS.
 * - posix_fadvise: no-op seguro.
 * - posix_memalign: malloc simple. El motor libera con free(), asi que
 *   alineacion 64B es optimizacion no requisito de correctitud (2.4).
 *
 * Nota: caracteres multibyte (o, n, a) evitados para no corromper el parseo
 * de comentarios bajo -std=gnu11 en gcc 16. */
#ifndef INGOT_UNISTD_SHIM_H
#define INGOT_UNISTD_SHIM_H

#include <io.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>  /* ssize_t */
#include <errno.h>
/* ssize_t: MinGW <sys/types.h> no siempre lo exporta sin __MINGW_USE_ANN. */
#ifndef _SSIZE_T_DEFINED
typedef intptr_t ssize_t;
#define _SSIZE_T_DEFINED
#endif

/* winsock2.h PRIMERO (antes de windows.h) para los wrappers de socket read/write/close. */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>

#define _SC_NPROCESSORS_ONLN 1001
#define POSIX_FADV_DONTNEED 4

#ifdef __cplusplus
extern "C" {
#endif

static long sysconf(int name) {
    if (name == _SC_NPROCESSORS_ONLN) {
        const char *env = getenv("NUMBER_OF_PROCESSORS");
        return env ? atoi(env) : 1;
    }
    return -1;
}

static ssize_t pread(int fd, void *buf, size_t n, long long off) {
    if (_lseeki64(fd, off, SEEK_SET) == -1) return -1;
    return _read(fd, buf, (unsigned int)n);
}

static int posix_fadvise(int fd, long long off, long long len, int adv) {
    (void)fd; (void)off; (void)len; (void)adv;
    return 0;
}

static int setenv(const char *name, const char *val, int overwrite) {
    (void)overwrite;
    size_t ln = strlen(name), vl = strlen(val ? val : "");
    char *s = (char *)malloc(ln + 1 + vl + 1);
    if (!s) return -1;
    memcpy(s, name, ln); s[ln] = '=';
    memcpy(s + ln + 1, val ? val : "", vl + 1);
    return _putenv(s) == 0 ? 0 : -1;
}
static int unsetenv(const char *name) {
    return setenv(name, "", 1);
}

/* malloc simple. Alineacion no requisito de correctitud (plan 2.4).
 * free() sobre el puntero es valido. */
static int posix_memalign(void **memptr, size_t alignment, size_t size) {
    (void)alignment;
    void *p = malloc(size ? size : 1);
    if (!p) return -1;
    *memptr = p;
    return 0;
}

/* read/write/close sobre sockets (qwen_tts_server.c): en winsock, SOCKET es un
 * UINT_PTR distinto de los file descriptors de CRT. La funcion CRT read()/write()
 * de <io.h> no opera sobre SOCKET -> wrapper: si el fd es un SOCKET valido
 * (no es INVALID_SOCKET), usa recv/recv/send/closesocket. */
static ssize_t mingw_read(int fd, void *buf, size_t n) {
    /* Si es un socket winsock, recv. Si es un fd de archivo CRT, _read. */
    if ((unsigned int)fd != (unsigned int)(long)(intptr_t)INVALID_SOCKET) {
        /* Heuristica: SOCKET en win32 tiene el bit altillo set en handles de
         * socket; el CRT fd es un entero pequeno (>=0, < 2096). Un SOCKET valido
         * es un entero positivo tambien — distincion confiable: usar __tryer
         * _get_osfhandle para verificar. Aqui: si _get_osfhandle falla, asumir
         * socket y usar recv. */
        intptr_t osf = _get_osfhandle(fd);
        if (osf == -1 && errno == EBADF) {
            /* fd invalido — probablemente un wrapped SOCKET que no es CRT fd.
             * Pero en MinGW, accept() devuelve SOCKET (UINT_PTR). El codigo
             * almacena socket_fd como 'int'. Esto es un gap: el codigo usa
             * int para socket_fd en winsock, donde SOCKET puede ser (UINT_PTR)-1
             * = INVALID_SOCKET. Usar recv directamente sobre (SOCKET)fd. */
            return (ssize_t)recv((SOCKET)fd, (char *)buf, (int)n, 0);
        }
    }
    return (ssize_t)_read(fd, buf, (unsigned int)n);
}
static ssize_t mingw_write(int fd, const void *buf, size_t n) {
    intptr_t osf = _get_osfhandle(fd);
    if (osf == -1 && errno == EBADF) {
        return (ssize_t)send((SOCKET)fd, (const char *)buf, (int)n, 0);
    }
    return (ssize_t)_write(fd, buf, (unsigned int)n);
}
static int mingw_close(int fd) {
    intptr_t osf = _get_osfhandle(fd);
    if (osf == -1 && errno == EBADF) {
        return closesocket((SOCKET)fd);
    }
    return _close(fd);
}

/* Redefinir read/write/close para el codigo server: enviamos estos wrappers
 * como funciones inline; el header posix_shim_win.h / este stub redefinira
 * los simbolos POSIX al wrapper de socket-compat. */
#define read(f,b,n) mingw_read((f),(b),(n))
#define write(f,b,n) mingw_write((f),(b),(n))
#define close(f) mingw_close((f))

#ifdef __cplusplus
}
#endif
#endif
