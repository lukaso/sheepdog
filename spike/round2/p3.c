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
#include <stdint.h>
extern char **environ;
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a,b; uint64_t r2,r3; };
static int (*resp)(pid_t); static int (*disc)(posix_spawnattr_t*,int); static uint64_t (*respu)(pid_t);
static uint64_t U(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.uniq:0; }
static uint64_t PU(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.puniq:0; }
static void rep(const char*t,pid_t p){ printf("%-34s pid=%-6d resp=%-6d respuniq=%llu uniq=%llu puniq=%llu\n",t,p,resp(p),(unsigned long long)respu(p),(unsigned long long)U(p),(unsigned long long)PU(p)); fflush(stdout);}
int main(int argc,char**argv){
 alarm(20);
 resp=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 respu=dlsym(RTLD_DEFAULT,"responsibility_get_uniqueid_responsible_for_pid");
 disc=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 if(argc>1){ /* root: leave a setsid double-fork stray, and a plain child stray, exit */
   int fd[2]; pipe(fd); pid_t c=fork(); if(c==0){ setsid(); pid_t g=fork(); if(g==0){ close(fd[0]);close(fd[1]); usleep(100000); execl("/bin/sleep","sleep","6",(char*)0);} write(fd[1],&g,sizeof g); _exit(0);}
   pid_t g; read(fd[0],&g,sizeof g); waitpid(c,0,0);
   pid_t k=fork(); if(k==0){ close(fd[0]);close(fd[1]); execl("/bin/sleep","sleep","6",(char*)0);}
   printf("STRAYS %d %d\n",g,k); fflush(stdout); usleep(300000); return 0; }
 int fd[2]; pipe(fd); fcntl(fd[0],F_SETFD,FD_CLOEXEC); fcntl(fd[1],F_SETFD,FD_CLOEXEC);
 posix_spawn_file_actions_t fa; posix_spawn_file_actions_init(&fa); posix_spawn_file_actions_adddup2(&fa,fd[1],1);
 posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); pid_t p; char*rv[]={argv[0],"root",NULL};
 posix_spawn(&p,argv[0],&fa,&a,rv,environ); close(fd[1]);
 char buf[256]; int n=read(fd[0],buf,255); buf[n>0?n:0]=0; pid_t g=0,k=0; sscanf(buf,"STRAYS %d %d",&g,&k);
 rep("root alive",p); rep(" stray dblfork (root alive)",g); rep(" stray child (root alive)",k);
 siginfo_t si; waitid(P_PID,p,&si,WEXITED|WNOWAIT);
 rep("root ZOMBIE (not reaped)",p); rep(" stray dblfork (root zombie)",g); rep(" stray child (root zombie)",k);
 waitpid(p,0,0);
 rep("root REAPED",p); rep(" stray dblfork (root reaped)",g); rep(" stray child (root reaped)",k);
 kill(g,9); kill(k,9); return 0;}
