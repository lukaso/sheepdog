#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/wait.h>
static pid_t child; static int mode;
static void ch(int s){ usleep(300000); /* "restore the terminal" */ int f=open("/private/tmp/claude-501/dk-review4/restored",O_CREAT|O_WRONLY,0600); close(f); signal(SIGTSTP,SIG_DFL); raise(SIGTSTP); signal(SIGTSTP,ch);}
static void sup(int s){ if(mode) kill(child,SIGSTOP); raise(SIGSTOP); }
int main(int c,char**v){ mode=atoi(v[1]); pid_t g=fork();
 if(!g){ setpgid(0,0); child=fork(); if(!child){ signal(SIGTSTP,ch); for(;;) pause(); }
   signal(SIGTSTP,sup); for(;;) pause(); }
 setpgid(g,g); FILE*f=fopen("/private/tmp/claude-501/dk-review4/pids","a"); fprintf(f,"%d\n",g); fclose(f);
 usleep(200000); unlink("/private/tmp/claude-501/dk-review4/restored"); killpg(g,SIGTSTP);   /* what the tty does on ^Z */
 usleep(800000); printf("mode=%s restored-marker=%s\n", mode?"supervisor SIGSTOPs members":"control: no SIGSTOP", access("/private/tmp/claude-501/dk-review4/restored",F_OK)==0?"YES":"NO (stopped mid-handler)");
 killpg(g,SIGKILL); killpg(g,SIGCONT); waitpid(g,0,0); return 0;}
