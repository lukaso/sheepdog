#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <dlfcn.h>
#include <sys/wait.h>
#include <stdint.h>
extern char **environ;
static int (*resp)(pid_t); static int (*disc)(posix_spawnattr_t*,int);
int main(int argc,char**argv){
 alarm(20);
 resp=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 disc=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 if(argc>1 && !strcmp(argv[1],"sup")){
   printf("after SETEXEC: pid=%d resp(self)=%d\n",getpid(),resp(getpid())); fflush(stdout);
   /* spawn root WITHOUT disclaim; root leaves a stray and exits */
   int fd[2]; pipe(fd); pid_t r=fork(); if(r==0){ pid_t c=fork(); if(c==0){ setsid(); pid_t g=fork(); if(g==0){ close(fd[0]);close(fd[1]); usleep(50000); execl("/bin/sleep","sleep","5",(char*)0);} write(fd[1],&g,sizeof g); _exit(0);} waitpid(c,0,0); _exit(0);}
   pid_t g; read(fd[0],&g,sizeof g); printf("root=%d resp(root)=%d\n",r,resp(r)); waitpid(r,0,0); usleep(200000);
   printf("root reaped; stray %d resp=%d (sup=%d)\n",g,resp(g),getpid()); kill(g,9); return 0; }
 printf("launcher pid=%d resp(self)=%d\n",getpid(),resp(getpid())); fflush(stdout);
 posix_spawnattr_t a; posix_spawnattr_init(&a); int d=disc(&a,1); posix_spawnattr_setflags(&a,POSIX_SPAWN_SETEXEC);
 char*sv[]={argv[0],"sup",NULL}; pid_t p; int rc=posix_spawn(&p,argv[0],NULL,&a,sv,environ);
 printf("SETEXEC failed rc=%d disc=%d\n",rc,d); return 1;}
