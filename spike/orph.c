#include <stdio.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>
#include <stdlib.h>
int main(){ int fd[2]; pipe(fd);
 pid_t p=fork(); if(p==0){ setsid(); pid_t c=fork(); if(c==0){ raise(SIGSTOP); sleep(3); _exit(0);} write(fd[1],&c,sizeof c); usleep(200000); _exit(0);} 
 pid_t c; read(fd[0],&c,sizeof c); waitpid(p,0,0); usleep(300000);
 char cmd[64]; snprintf(cmd,64,"ps -o pid,stat,ppid -p %d",c); system(cmd); int alive = kill(c,0)==0; printf("stopped child after parent death: alive=%d\n",alive); if(alive) kill(c,9); return 0;}
