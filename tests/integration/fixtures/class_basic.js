// @expected-stdout: 7
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  sum() {
    return this.x + this.y;
  }
}
let p = new Point(3, 4);
console.log(p.sum());
