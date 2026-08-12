/* sys/time.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El motor incluye <sys/time.h> para struct timeval + gettimeofday.
 * MinGW-w64 en realtivo provee gettimeofday (como mingw_gettimeofday)
 * y struct timeval en <winsock2.h>, expuesto con _GNU_SOURCE.
 *
 * Este stub NO vive en el sysroot de mingw, asi que usamos #include_next
 * para encadenar al header real de MinGW (que define struct timezone
 * + mingw_gettimeofday), y luego alias gettimeofday sobre mingw_gettimeofday
 * por si _GNU_SOURCE no expone el alias en algun toolchain. */
#ifndef INGOT_SYS_TIME_SHIM_H
#define INGOT_SYS_TIME_SHIM_H

/* winsock2.h (ya incluido por sys/socket.h en server.c) define struct timeval.
 * struct timeval tambien esta disponible via _mingw.h. Incluir winsock2.h aqui
 * asegura timeval exista antes del include_next que lo referencia. */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>

/* Encadenar al header real de MinGW para struct timezone + mingw_gettimeofday. */
#include_next <sys/time.h>

/* Alias explicito: en algunos toolchains _GNU_SOURCE no renamea mingw_gettimeofday
 * a gettimeofday. Declararlo si no existe. */
#ifdef __MINGW32__
#ifndef gettimeofday
extern int mingw_gettimeofday(struct timeval *__restrict__, struct timezone *__restrict__);
/* Wrapper inline: gcc no permite redefinir una funcion como macro si ya es una
 * funcion del runtime; usar una static inline con nombre distinto NO sirve (el
 * codigo llama 'gettimeofday'). En su lugar: redirigir via --def de mingw.
 * Simplificacion: MinGW UCRT64 ya expone 'gettimeofday' con _GNU_SOURCE, asi que
 * aqui no redefinimos; si falla, el build real reporta y se usa mingw_gettimeofday
 * con -Dgettimeofday=mingw_gettimeofday. */
#endif
#endif

#endif /* INGOT_SYS_TIME_SHIM_H */
