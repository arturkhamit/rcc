#include <stdio.h>
#include <string.h>
int calls;
int next(void) { return ++calls; }
long sum2(int n, int m, int a[n][m]) { long s = 0; for (int i = 0; i < n; i++) for (int j = 0; j < m; j++) s += a[i][j] * (i + 1); return s; }
void fill(int n, int m, double a[][m], double v) { for (int i = 0; i < n; i++) for (int j = 0; j < m; j++) a[i][j] = v + i * m + j; }
double trace(int n, double a[n][n]) { double t = 0; for (int i = 0; i < n; i++) t += a[i][i]; return t; }
int t3b(int s, int arr[s][3][4]) { return arr[1][2][3]; }
int t3c(int s, int arr[2][s][4]) { return arr[1][2][3] + (int)sizeof(arr[0]); }
int t3d(int s, int arr[2][3][s]) { return arr[1][2][3] + (int)sizeof(*arr); }
int t3e(int s, int arr[][3][--s]) { return (int)sizeof arr + s * 1000 + arr[1][2][3]; }
int side(int i, int array[i++]) { return i; }
int side2(int a, int b[a++], int c, int d[c++]) { return a * 10 + c; }
void proto(int n, int a[*][*]);
void proto(int n, int a[n][n * 2]) { a[n - 1][n * 2 - 1] = 77; }
int ptr_vla(int n, int (*p)[n]) { return (int)sizeof(*p) + (*p)[n - 1]; }
int main(void) {
  int a[3][4], b[2][3][4], c[5][10];
  double d[4][4];
  for (int i = 0; i < 3; i++) for (int j = 0; j < 4; j++) a[i][j] = i * 4 + j;
  printf("%ld\n", sum2(3, 4, a));
  fill(4, 4, d, 0.5);
  printf("%g %g\n", trace(4, d), d[3][2]);
  memset(b, 0, sizeof b); b[1][2][3] = 123;
  printf("%d %d %d %d\n", t3b(2, b), t3c(3, b), t3d(4, b), t3e(5, b));
  printf("%d %d\n", side(10, a[0]), side2(1, a[0], 1, a[1]));
  proto(5, c); printf("%d\n", c[4][9]);
  int row[6] = { 1, 2, 3, 4, 5, 6 };
  printf("%d\n", ptr_vla(6, &row));
  int n = 3, m = 5, v[2][m], w[n][2][m];
  printf("%zu %zu %zu %zu\n", sizeof v, sizeof v[0], sizeof w, sizeof w[1]);
  v[1][4] = 9; w[2][1][4] = 8;
  printf("%d %d\n", v[1][4], w[2][1][4]);
  return 0;
}
