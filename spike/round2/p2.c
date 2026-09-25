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
static int (*resp)(pid_t); static int (*disc)(posix_spawnattr_t*,int); static int (*respu)(pid_t,uint64_t*);
static uint64_t U(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.uniq:0; }
static uint64_t PU(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.puniq:0; }
static void rep(const char*t,pid_t p){ uint64_t ru=0; int rr=respu(p,&ru); printf("%-32s pid=%-6d resp=%-6d respuniq=%llu(rc=%d) uniq=%llu puniq=%llu ppid=%d\n",t,p,resp(p),(unsigned long long)ru,rr,(unsigned long long)U(p),(unsigned long long)PU(p),({struct proc_bsdinfo b; proc_pidinfo(p,PROC_PIDTBSDINFO,0,&b,sizeof b)>0?(int)b.pbi_ppid:-1;})); fflush(stdout);}
int main(int argc,char**argv){
 alarm(20);
 resp=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 respu=dlsym(RTLD_DEFAULT,"responsibility_get_uniqueid_responsible_for_pid");
 disc=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 if(argc>1 && !strcmp(argv[1],"inner")){
   int fd[2]; pipe(fd); pid_t c=fork(); if(c==0){ setsid(); pid_t g=fork(); if(g==0){ int n=open("/dev/null",O_RDWR); dup2(n,1); dup2(n,2); close(fd[0]); close(fd[1]); usleep(150000); execl("/bin/sleep","sleep","5",(char*)0);} write(fd[1],&g,sizeof g); if(!getenv("FASTEXIT")) usleep(300000); _exit(0);}
   pid_t g; read(fd[0],&g,sizeof g); usleep(50000);
   rep("  gc while its parent alive",g);
   waitpid(c,0,0); usleep(50000);
   rep("  gc after parent exited",g); usleep(200000); rep("  gc after its exec",g);
   printf("GC %d\n",g); fflush(stdout); return 0; }
 if(argc>1 && !strcmp(argv[1],"root")){
   rep(" root self",getpid());
   posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); char*iv[]={argv[0],"inner",NULL};
   int fd[2]; pipe(fd); fcntl(fd[0],F_SETFD,FD_CLOEXEC); fcntl(fd[1],F_SETFD,FD_CLOEXEC); posix_spawn_file_actions_t fa; posix_spawn_file_actions_init(&fa); posix_spawn_file_actions_adddup2(&fa,fd[1],1);
   pid_t p; posix_spawn(&p,argv[0],&fa,&a,iv,environ); close(fd[1]); pid_t inner=p;
   usleep(20000); rep(" inner self (from root)",inner);
   char buf[4096]; int n=0,r; while((r=read(fd[0],buf+n,sizeof buf-1-n))>0) n+=r; buf[n]=0;
   fputs(buf,stdout); rep(" inner zombie (unreaped)",inner); waitpid(p,0,0);
   pid_t gc=atoi(strstr(buf,"GC ")+3);
   rep(" gc AFTER inner reaped",gc);
   kill(gc,9);
   return 0; }
 posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); pid_t p; char*rv[]={argv[0],"root",NULL};
 posix_spawn(&p,argv[0],NULL,&a,rv,environ); waitpid(p,0,0);
 return 0;}
