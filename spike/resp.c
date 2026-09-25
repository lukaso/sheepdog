#include <stdio.h>
#include <unistd.h>
#include <signal.h>
#include <spawn.h>
#include <dlfcn.h>
#include <libproc.h>
#include <sys/wait.h>
extern char **environ;
int main(int argc,char**argv){
 int (*resp)(pid_t)=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 int (*disc)(posix_spawnattr_t*,int)=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 if(argc>1){ /* inner: double fork + setsid, report grandchild pid */
   int fd[2]; pipe(fd); pid_t c=fork(); if(c==0){ setsid(); pid_t g=fork(); if(g==0){ execl("/bin/sleep","sleep","3",(char*)0);} write(fd[1],&g,sizeof g); _exit(0);} 
   pid_t g; read(fd[0],&g,sizeof g); waitpid(c,0,0); usleep(100000);
   printf("inner pid=%d resp(inner)=%d  gc=%d ppid? resp(gc)=%d\n",getpid(),resp(getpid()),g,resp(g));
   kill(g,9); return 0; }
 posix_spawnattr_t a; posix_spawnattr_init(&a); int dr=disc(&a,1); pid_t p; char*av[]={argv[0],"inner",NULL};
 int r=posix_spawn(&p,argv[0],NULL,&a,av,environ); printf("disclaim rc=%d spawn rc=%d child=%d resp(me)=%d\n",dr,r,p,resp(getpid()));
 waitpid(p,0,0);
 /* control without disclaim */
 posix_spawnattr_t b; posix_spawnattr_init(&b); posix_spawn(&p,argv[0],NULL,&b,av,environ); printf("control child=%d\n",p); waitpid(p,0,0);
 return 0;}
