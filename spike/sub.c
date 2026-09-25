#include <stdio.h>
#include <unistd.h>
#include <sys/prctl.h>
#include <sys/wait.h>
int main(int argc,char**v){ int on=argc>1; if(on) prctl(PR_SET_CHILD_SUBREAPER,1,0,0,0);
 int fd[2]; pipe(fd);
 pid_t c=fork(); if(c==0){ pid_t g=fork(); if(g==0){ setsid(); sleep(2); _exit(0);} write(fd[1],&g,sizeof g); _exit(0);}
 pid_t g; read(fd[0],&g,sizeof g); waitpid(c,0,0); usleep(300000);
 char p[64]; snprintf(p,64,"/proc/%d/stat",g); FILE*f=fopen(p,"r"); int pid,pp; char comm[64],st; fscanf(f,"%d %s %c %d",&pid,comm,&st,&pp);
 printf("subreaper=%d  orphan(setsid) ppid=%d  (me=%d)\n",on,pp,getpid()); return 0;}
