// @expected-stdout: 1,2,3
let range = {
  from: 1,
  to: 3,
  [Symbol.iterator]() {
    let cur = this.from;
    let last = this.to;
    return {
      next() {
        return cur <= last
          ? { value: cur++, done: false }
          : { done: true };
      }
    };
  }
};
let result = [];
for (let x of range) {
  result.push(x);
}
console.log(result.join(","));
