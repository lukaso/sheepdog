#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <string.h>
#include <termios.h>
int main(){ setsid(); int f=open("/dev/tty",O_RDWR); int e=errno; pid_t g=tcgetpgrp(0); int e2=errno;
 printf("no ctty: open /dev/tty -> %d (%s); tcgetpgrp(stdin) -> %d (%s)\n", f, strerror(e), (int)g, strerror(e2)); return 0;}
