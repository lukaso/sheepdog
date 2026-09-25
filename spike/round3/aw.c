#include "common.h"
int main(int c,char**v){ pid_t p=fork(); if(p==0){ execv(v[1],v+1); _exit(127);} printf("wrapper: child=%d\n",p); fflush(stdout); int st; pid_t w=waitpid(p,&st,0); printf("wrapper: waitpid=%d exited=%d code=%d\n",w,WIFEXITED(st),WEXITSTATUS(st)); return 0; }
