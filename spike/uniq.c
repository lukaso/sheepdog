#include <stdio.h>
#include <unistd.h>
#include <signal.h>
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/wait.h>
#include <stdint.h>
#define PROC_PIDUNIQIDENTIFIERINFO 17
struct proc_uniqidentifierinfo { uint8_t p_uuid[16]; uint64_t p_uniqueid; uint64_t p_puniqueid; int32_t p_idversion; int32_t p_orig_ppidversion; uint64_t p_reserve2; uint64_t p_reserve3; };
static void show(const char*l,pid_t p){ struct proc_uniqidentifierinfo u; struct proc_bsdinfo b;
 int r=proc_pidinfo(p,PROC_PIDUNIQIDENTIFIERINFO,0,&u,sizeof u); int r2=proc_pidinfo(p,PROC_PIDTBSDINFO,0,&b,sizeof b);
 printf("%-12s pid=%d r=%d uniq=%llu puniq=%llu ppid=%d pgid=%d\n",l,p,r,u.p_uniqueid,u.p_puniqueid,r2>0?b.pbi_ppid:-1,r2>0?b.pbi_pgid:-1);}
int main(void){ int fd[2]; pipe(fd);
 show("me",getpid());
 pid_t c=fork();
 if(c==0){ pid_t g=fork(); if(g==0){ setsid(); sleep(4); _exit(0);} write(fd[1],&g,sizeof g); sleep(1); _exit(0);} 
 pid_t g; read(fd[0],&g,sizeof g); usleep(100000);
 show("child",c); show("grandchild",g);
 sleep(2); waitpid(c,0,0);
 show("gc-orphaned",g);
 /* platform binary via exec */
 pid_t s=fork(); if(s==0){ execl("/bin/sleep","sleep","3",(char*)0);} usleep(200000); show("/bin/sleep",s);
 kill(g,9); kill(s,9); return 0;}
