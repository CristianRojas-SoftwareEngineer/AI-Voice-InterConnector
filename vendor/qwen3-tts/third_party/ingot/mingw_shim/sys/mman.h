/* sys/mman.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El fork qwen3-tts incluye sys/mman.h (no existe en MinGW) y llama
 * mmap, munmap, madvise, mas macros PROT_*, MAP_*, MAP_FAILED, MADV_*.
 *
 * Estilo del plan (PLAN DE MIGRACION 2.4): shims POSIX acotados.
 * El motor abre el file .bin con open() (MinGW lo provee) y llama mmap
 * read-only (PROT_READ|MAP_PRIVATE). Aqui mmap devuelve MAP_FAILED; el
 * motor verifica s-map == MAP_FAILED y cae a su ruta pread() definida
 * en el stub unistd.h. */
#ifndef INGOT_MMAN_SHIM_H
#define INGOT_MMAN_SHIM_H

/* mmap real en Windows (PLAN §2.4: mmap→CreateFileMapping+MapViewOfFile).
 * El ingot abre el shard con open() (MinGW) y pasa `int fd`; aquí se convierte
 * a HANDLE vía _get_osfhandle y se mapea read-only. El ingot mapea cada shard
 * con off=0, así que basta MapViewOfFile sin slot-table (los offsets != 0
 * requerirían alineación a 64KB; no ocurren en este layout de shards). */
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <io.h>
#include <errno.h>

#define PROT_READ  0x1
#define PROT_WRITE 0x2
#define PROT_NONE  0x0
#define MAP_PRIVATE 0x02
#define MAP_FAILED ((void *)-1)

#define MADV_DONTNEED 4
#define POSIX_MADV_DONTNEED 4

static void *mmap(void *addr, long len, int prot, int flags, int fd, long long off) {
    (void)addr; (void)flags;
    if (fd < 0) { errno = EBADF; return MAP_FAILED; }
    HANDLE hFile = (HANDLE)_get_osfhandle(fd);
    if (hFile == INVALID_HANDLE_VALUE || hFile == NULL) { errno = EBADF; return MAP_FAILED; }
    DWORD  protect = (prot & PROT_WRITE) ? PAGE_READWRITE : PAGE_READONLY;
    DWORD  access  = (prot & PROT_WRITE) ? FILE_MAP_WRITE : FILE_MAP_READ;
    HANDLE hmap = CreateFileMappingA(hFile, NULL, protect, 0, 0, NULL);
    if (!hmap) { errno = ENOENT; return MAP_FAILED; }
    /* offset debe ser múltiplo de allocation granularity; el ingot pasa off=0. */
    DWORD lo = (DWORD)off, hi = (DWORD)(((unsigned long long)off) >> 32);
    void *p = MapViewOfFile(hmap, access, hi, lo, (SIZE_T)len);
    CloseHandle(hmap);   /* el view mantiene el mapping vivo hasta UnmapViewOfFile */
    if (!p) { errno = ENOMEM; return MAP_FAILED; }
    return p;
}
static int munmap(void *addr, long len) {
    (void)len;
    return UnmapViewOfFile(addr) ? 0 : -1;
}
static int madvise(void *addr, long len, int adv) {
    (void)addr; (void)len; (void)adv; return 0;
}
static int posix_madvise(void *addr, long len, int adv) {
    (void)addr; (void)len; (void)adv; return 0;
}
#endif
