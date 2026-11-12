// Initializers, control flow, pointers, strings, statics, macros.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stddef.h>
#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <setjmp.h>

#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define STR(x) #x
#define XSTR(x) STR(x)
#define CAT(a, b) a##b
#define LOG(fmt, ...) printf(fmt "\n", ##__VA_ARGS__)
#define FOREACH(i, n) for (int i = 0; i < (n); i++)
#if defined(__x86_64__) && !defined(NOPE) && (1 + 1 == 2)
#define ARCH "x86_64"
#elif 0
#error no
#else
#define ARCH "other"
#endif

typedef struct Node { int val; struct Node *next; } Node;
typedef struct { const char *name; int (*fn)(int); int tab[3]; struct { int x, y; } pt; } Entry;
struct Flex { int n; int data[]; };

static int sq(int x) { return x * x; }
static int neg(int x) { return -x; }
static Entry table[] = {
    {"sq", sq, {1, 2, 3}, {4, 5}},
    {.name = "neg", .fn = neg, .tab = {[1] = 7}, .pt.y = 9},
    [3] = {"last"},
};
static int grid[3][4] = {{1, 2}, {5}, [2][1] = 8, 9};
static char msg[] = "hello", *ptrs[] = {"a", "bc", msg + 1};
static const char *names[4] = {[2] = "two", [0] = "zero"};
static int *gp = &grid[1][0], garr[] = {1, 2, 3}, glen = sizeof garr / sizeof *garr;
static double gd = 1.5 * 2, gf[2] = {1, 2.5};
static long goff = offsetof(Entry, pt) + sizeof(Node);
int counter(void) { static int n = 10; return n++; }
int fib(int n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
Node *push(Node *head, int v) { Node *n = malloc(sizeof *n); n->val = v; n->next = head; return n; }
static jmp_buf jb;
void thrower(int v) { if (v > 2) longjmp(jb, v); }

int classify(int c) {
    switch (c) {
    case 0: return 100;
    case 1: case 2: c += 10;
    case 3: c *= 2; break;
    case -5: return -1;
    default: c = 0;
    }
    return c;
}

int main(void) {
    LOG("%s %s %d %s", ARCH, XSTR(MAX(1, 2)), CAT(12, 34), __func__);
    LOG("plain");
    printf("%d %d %d\n", MAX(3, 7), __LINE__, (int)sizeof(__FILE__) > 1);
    FOREACH(i, 4) printf("%s:%d:%d ", table[i].name ? table[i].name : "-", table[i].fn ? table[i].fn(3) : 0, table[i].tab[1] + table[i].pt.y);
    printf("\n");
    FOREACH(i, 3) { FOREACH(j, 4) printf("%d ", grid[i][j]); }
    printf("\n%s %s %s %s %s %d %d %g %g %ld\n", msg, ptrs[1], ptrs[2], names[0], names[2], *gp, glen, gd, gf[1], goff);
    printf("%d %d %d\n", counter(), counter(), counter());
    printf("%d\n", fib(20));
    Node *list = 0; FOREACH(i, 5) list = push(list, i * i);
    int sum = 0; for (Node *n = list; n; n = n->next) sum += n->val;
    printf("%d\n", sum);
    FOREACH(i, 6) printf("%d ", classify(i - 1));
    printf("%d\n", classify(-5));
    // local initializers
    int a[5] = {1, 2, [4] = 5}, b[] = {[2] = 3, 4}, z[4] = {0};
    char s1[] = "abc", s2[8] = "xy", s3[3] = "abc";
    struct { int x; char c; double d; int arr[2]; } st = {.d = 2.5, .x = 1, .arr = {7, 8}}, st2 = st;
    Entry le = {"loc", .tab = {1}, .pt = {3, 4}}, *pe = &le;
    Node nd = {5}, *pn = &(Node){6, &nd};
    int *lit = (int[]){10, 20, 30};
    printf("%d %d %d %d %d %d %d %lu\n", a[1], a[3], a[4], b[2], b[3], z[3], lit[2], sizeof b);
    printf("%s %s %c %lu %lu\n", s1, s2, s3[2], sizeof s1, sizeof s2);
    printf("%d %d %g %d %d %s %d %d %d\n", st2.x, st2.c, st2.d, st2.arr[1], pe->pt.y, pe->name, pe->tab[0], pn->val, pn->next->val);
    // pointers
    int m[3][3] = {{1, 2, 3}, {4, 5, 6}, {7, 8, 9}};
    int (*row)[3] = m + 1, *p = &m[0][0], **pp = &p;
    printf("%d %d %d %d %ld %ld\n", (*row)[2], row[1][0], *(p + 4), (*pp)[8], &m[2][2] - p, (char *)&m[1] - (char *)m);
    p += 2; p--; printf("%d %d %d\n", *p++, *p, p[-1]);
    char *str = "The quick brown fox"; int vowels = 0;
    for (const char *q = str; *q; q++) if (strchr("aeiou", tolower(*q))) vowels++;
    printf("%d %d %s\n", vowels, (int)strlen(str), strstr(str, "brown"));
    void *vp = m; printf("%d\n", ((int *)vp)[4] + *(int *)((char *)vp + 8));
    // control flow
    int n = 0, t = 0;
    do { if (n == 3) { n++; continue; } t += n; n++; } while (n < 6);
    while (1) { if (++n > 10) break; }
    for (;;) { t++; if (t > 20) goto done; }
done:
    printf("%d %d\n", n, t);
    int cnt = 0; FOREACH(i, 10) { FOREACH(j, 10) { if (j > i) break; if ((i + j) % 2) continue; cnt++; } }
    printf("%d %d\n", cnt, (n > 5 ? n : t) + (0, 3));
    int v = setjmp(jb); if (v == 0) { thrower(1); thrower(7); } printf("%d\n", v);
    struct Flex *fl = malloc(sizeof *fl + 3 * sizeof(int)); fl->n = 3; fl->data[2] = 42;
    printf("%lu %d %.4f %g\n", sizeof(struct Flex), fl->data[2], sqrt(2.0), floor(-2.5) + pow(2, 10) + fabs(-1.5));
    errno = 0; strtol("99999999999999999999", 0, 10); printf("%d\n", errno == ERANGE);
    char buf[32]; memset(buf, 'x', 5); buf[5] = 0; memcpy(buf + 1, "ab", 2); printf("%s %d\n", buf, memcmp(buf, "xab", 3));
    free(fl);
    return sum == 30 ? 0 : 1;
}
