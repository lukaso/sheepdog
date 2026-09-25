#include "common.h"
int main(){ init(); uint64_t prev=U(getpid()); int bad=0; for(int i=0;i<300;i++){ pid_t p=fork(); if(!p) _exit(0); uint64_t u=U(p); waitpid(p,0,0); if(u && u<=prev) bad++; if(u) prev=u; } printf("300 forks: non-increasing uniqueids=%d last=%llu sizeof=%zu\n",bad,(unsigned long long)prev,sizeof(prev)); }
