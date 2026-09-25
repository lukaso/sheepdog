#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <spawn.h>
#include <dlfcn.h>
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/wait.h>
#include <sys/sysctl.h>
#include <stdint.h>
extern char **environ;
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a; int32_t b; uint64_t r2,r3; };
static int get(pid_t p, struct uq*u){ return proc_pidinfo(p,17,0,u,sizeof *u); }
static void show(const char*l,pid_t p){ struct uq u={0}; int r=get(p,&u); struct proc_bsdinfo b; int r2=proc_pidinfo(p,PROC_PIDTBSDINFO,0,&b,sizeof b);
 printf("%-14s pid=%d r=%d uniq=%llu puniq=%llu bsd_r=%d status=%d\n",l,p,r,u.uniq,u.puniq,r2,r2>0?b.pbi_status:-1);}
int main(void){
 show("launchd",1); show("me",getpid());
 /* exec preserves uniq? child sleeps, then execs /bin/sleep */
 int fd[2]; pipe(fd);
 pid_t c=fork(); if(c==0){ char x; read(fd[0],&x,1); execl("/bin/sleep","sleep","2",(char*)0); _exit(1);} 
 usleep(100000); show("pre-exec",c); write(fd[1],"x",1); usleep(200000); show("post-exec",c); kill(c,9); waitpid(c,0,0);
 /* posix_spawn */
 pid_t s; char*av[]={"/bin/sleep","2",NULL}; posix_spawn(&s,"/bin/sleep",NULL,NULL,av,environ); usleep(100000); show("posix_spawn",s); kill(s,9); waitpid(s,0,0);
 /* vfork */
 pid_t v=vfork(); if(v==0){ execl("/bin/sleep","sleep","2",(char*)0); _exit(1);} usleep(100000); show("vfork+exec",v); kill(v,9); waitpid(v,0,0);
 /* zombie */
 pid_t z=fork(); if(z==0) _exit(0); usleep(200000); show("zombie",z);
 int n=proc_listallpids(NULL,0); pid_t *pl=malloc(sizeof(pid_t)*(n+100)); n=proc_listallpids(pl,sizeof(pid_t)*(n+100)); int inl=0; for(int i=0;i<n;i++) if(pl[i]==z) inl=1; printf("zombie in listallpids=%d\n",inl);
 waitpid(z,0,0); show("reaped",z);
 /* responsibility API */
 void*f=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid"); printf("responsibility sym=%p\n",f);
 if(f){ int (*g)(pid_t)=f; printf("resp(me)=%d resp(1)=%d getppid=%d\n",g(getpid()),g(1),getppid()); }
 void*d=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim"); printf("disclaim sym=%p\n",d);
 struct timeval bt; size_t sz=sizeof bt; sysctlbyname("kern.boottime",&bt,&sz,NULL,0); printf("boottime=%ld uptime_s=%ld\n",bt.tv_sec,(long)(time(NULL)-bt.tv_sec));
 return 0;}
