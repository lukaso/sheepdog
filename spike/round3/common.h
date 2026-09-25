#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <dlfcn.h>
#include <libproc.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <stdint.h>
#include <errno.h>
#include <termios.h>
extern char **environ;
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a,b; uint64_t r2,r3; };
static int (*resp)(pid_t); static int (*disc)(posix_spawnattr_t*,int);
static uint64_t U(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.uniq:0; }
static uint64_t PU(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.puniq:0; }
static int RU(pid_t p){ int r=resp(p); return r; }
static void init(void){ resp=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid"); disc=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim"); }
static void rep(const char*t,pid_t p){ printf("%-30s pid=%-6d resp=%-6d uniq=%llu puniq=%llu\n",t,p,resp(p),(unsigned long long)U(p),(unsigned long long)PU(p)); fflush(stdout);}
