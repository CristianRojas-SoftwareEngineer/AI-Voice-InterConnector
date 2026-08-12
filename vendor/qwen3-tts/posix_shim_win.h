/* posix_shim_win.h — compat POSIX mínima para compilar el fork qwen3-tts bajo
 * MinGW-w64 / UCRT64. El motor asume Linux (mmap/pread/posix_memalign/
 * posix_madvise/posix_fadvise/setenv). Define símbolos ausentes + macros
 * (PROT_*/MAP_*/MAP_FAILED) usando Win32, sin ifdefs en el resto del código.
 *
 * Cobertura empírica (grep del fork): mmap (read-only, PROT_READ|MAP_PRIVATE),
 * munmap, madvise/POSIX_MADV_DONTNEED, posix_fadvise POSIX_FADV_DONTNEED,
 * pread(off_t), posix_memalign (+ free), setenv/unsetenv, sysconf(_SC_..).
 *
 * NOTA de inclusión: <windows.h> se incluye al FINAL (después de los headers
 * base de MinGW) porque vadefs.h necesita __GNUC_va_list definido por _mingw.h
 * antes de que windows.h lo toque. Incluirlo arriba rompe stdio.h (ver error
 * __gnuc_va_list en sys/stdio64). El motor ya incluye <windows.h> donde hace
 * falta; aquí declaramos forward-decls de los Win32 que usamos.
 */
#ifndef POSIX_SHIM_WIN_H
#define POSIX_SHIM_WIN_H

/* Orden crítico de includes (validado con gcc/MinGW-UCRT64):
 *   1. <windows.h> PRIMERO con WIN32_LEAN_AND_MEAN — pulliza _mingw.h que
 *      define __GNUC_va_list ANTES del vadefs.h de windows.h. Incluirlo
 *      después de <io.h> rompe stdio.h (_vfscanf_s_l).
 *   2. Luego headers base de MinGW (ssize_t, off_t, sys/types). */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <io.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* — Protección / flags de mmap: el motor solo usa PROT_READ + MAP_PRIVATE. — */
#define PROT_READ  0x1
#define PROT_WRITE 0x2
#define PROT_NONE  0x0
#define MAP_PRIVATE 0x02
#define MAP_FAILED ((void *)-1)

/* madvise / posix_madvise: solo MADV_DONTNEED / POSIX_MADV_DONTNEED aparecen.
 * En Win32 el descargue de páginas map-file-backed se controla por
 * MEM_RELEASE sobre el manejador de archivo; aquí es un no-op seguro. */
#define MADV_DONTNEED        4
#define POSIX_MADV_DONTNEED  4
#define POSIX_FADV_DONTNEED  4

/* getenv/setenv: setenv/unsetenv no existen en MinGW. setenv(2) sobre heap
 * copiado; el motor llama setenv con cadena literal → seguro. */
static int setenv(const char *name, const char *value, int overwrite) {
    if (!name || !*name) return -1;
    if (!overwrite && getenv(name)) return 0;
    size_t l = strlen(name);
    size_t v = strlen(value ? value : "");
    char *buf = (char *)malloc(l + 1 + v + 1);
    if (!buf) return -1;
    memcpy(buf, name, l); buf[l] = '=';
    memcpy(buf + l + 1, value ? value : "", v + 1);
    return _putenv(buf) == 0 ? 0 : -1;   /* _putenv copia internamente */
}
static int unsetenv(const char *name) {
    return setenv(name, "", 1);
}

/* sysconf: el motor consulta _SC_NPROCESSORS_ONLN (número de CPUs). */
#define _SC_NPROCESSORS_ONLN 1001  /* valor arbitrario: sólo usamos este caso */
static long sysconf(int name) {
    if (name == _SC_NPROCESSORS_ONLN) {
        SYSTEM_INFO si; GetSystemInfo(&si);
        return (long)si.dwNumberOfProcessors;
    }
    return -1;
}

/* pread: MinGW no expone pread. Emular sobre SetFilePointerEx + ReadFile
 * preservando el offset del file descriptor (thread-safe en el sentido de no
 * mutar la posición del fd compartido). */
static ssize_t pread(int fd, void *buf, size_t n, off_t off) {
    HANDLE h = (HANDLE)_get_osfhandle(fd);
    if (h == INVALID_HANDLE_VALUE) { errno = EBADF; return -1; }
    LARGE_INTEGER li; li.QuadPart = off;
    if (!SetFilePointerEx(h, li, NULL, FILE_BEGIN)) return -1;
    DWORD got = 0;
    if (!ReadFile(h, buf, (DWORD)n, &got, NULL)) return -1;
    return (ssize_t)got;
}

/* mmap: el motor solo mapea read-only (PROT_READ|MAP_PRIVATE, offset 0, len=filesize).
 * Win32: CreateFileMapping + MapViewOfFile. Se devuelve una estructura oculta
 * para poder munmap. El puntero devuelto NO es el de la vista (se usa un
 * slot de handle interno), pero el motor nunca inspecciona el contenido del
 * puntero fuera de tratarlo como void*; sólo mantiene + munmap. */
#define MMAP_SHIM_SLOTS 256
typedef struct { HANDLE fh; HANDLE mh; void *addr; size_t len; } mmap_slot;
static mmap_slot mmap_slots[MMAP_SHIM_SLOTS];
static CRITICAL_SECTION mmap_cs;
static int mmap_cs_init = 0;

static void *mmap(void *addr, size_t len, int prot, int flags, int fd, off_t off) {
    if (!mmap_cs_init) { InitializeCriticalSection(&mmap_cs); mmap_cs_init = 1; }
    (void)addr; (void)prot; (void)flags;
    HANDLE h = (HANDLE)_get_osfhandle(fd);
    if (h == INVALID_HANDLE_VALUE || h == NULL) return MAP_FAILED;
    LARGE_INTEGER size; size.QuadPart = off + (LONGLONG)len;
    HANDLE mh = CreateFileMappingA(h, NULL, PAGE_READONLY, size.HighPart, size.LowPart, NULL);
    if (!mh) return MAP_FAILED;
    void *view = MapViewOfFile(mh, FILE_MAP_READ, 0, 0, len);
    if (!view) { CloseHandle(mh); return MAP_FAILED; }
    EnterCriticalSection(&mmap_cs);
    void *ret = MAP_FAILED;
    for (int i = 0; i < MMAP_SHIM_SLOTS; i++) {
        if (mmap_slots[i].addr == NULL) {
            mmap_slots[i].fh = h; mmap_slots[i].mh = mh;
            mmap_slots[i].addr = view; mmap_slots[i].len = len;
            ret = view; break;
        }
    }
    LeaveCriticalSection(&mmap_cs);
    if (ret == MAP_FAILED) { UnmapViewOfFile(view); CloseHandle(mh); }
    return ret;
}

static int munmap(void *addr, size_t len) {
    (void)len;
    if (!mmap_cs_init) return -1;
    EnterCriticalSection(&mmap_cs);
    int rc = -1;
    for (int i = 0; i < MMAP_SHIM_SLOTS; i++) {
        if (mmap_slots[i].addr == addr) {
            UnmapViewOfFile(mmap_slots[i].addr);
            CloseHandle(mmap_slots[i].mh);
            mmap_slots[i].addr = NULL; rc = 0; break;
        }
    }
    LeaveCriticalSection(&mmap_cs);
    return rc;
}

/* madvise: el motor sólo mapea DONTNEED → hint. No-op seguro. */
static int madvise(void *addr, size_t len, int adv) { (void)addr; (void)len; (void)adv; return 0; }
static int posix_madvise(void *addr, size_t len, int adv) { (void)addr; (void)len; (void)adv; return 0; }
static int posix_fadvise(int fd, off_t off, off_t len, int adv) { (void)fd; (void)off; (void)len; (void)adv; return 0; }

/* posix_memalign: MinGW no provee alineación > 16 con malloc. Usar
 * VirtualAlloc (granularidad de sistema, 64 KiB ≈ alineado a 64). El motor
 * libera con free(); VirtualAlloc-backed no es free()-compatible → el caller
 * en ingot usa ingot_aligned_free(){ free(ptr) }. Ver NOTA de bloqueo más abajo.
 */
/* posix_memalign: ni msvcrt ni UCRT exportan aligned_alloc, y el motor/ingot
 * libera estos buffers con free() plano (ingot_aligned_free(){ free(ptr) }).
 * _aligned_malloc NO es free()-compatible → descartado por §2.4 del plan.
 * VirtualAlloc NO es free()-compatible → requeriría editar cpu.c.
 * El plan (PLAN-DE-MIGRACIÓN §2.4 ítem 2) resuelve esto: caer a malloc simple,
 * perdiendo la alineación a 64B como OPTIMIZACIÓN, no como requisito de
 * correctitud para el motor (buffers de trabajo SIMD, no estructuras de datos
 * con requisitos de packing). free() sobre heap malloc es válido. */
static int posix_memalign(void **memptr, size_t alignment, size_t size) {
    (void)alignment;   /* intencional: malloc garantiza ≤16B; plan lo acepta */
    void *p = malloc(size ? size : 1);
    if (!p) return ENOMEM;
    *memptr = p; return 0;
}

#ifdef __cplusplus
}
#endif
#endif /* POSIX_SHIM_WIN_H */
