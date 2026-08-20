// @expected-stdout: 10,20,30
let obj = {
  [Symbol.iterator]() {
    let values = [10, 20, 30];
    let index = 0;
    return {
      next() {
        if (index < values.length) {
          return { value: values[index++], done: false };
        }
        return { done: true };
      }
    };
  }
};
let result = [];
for (let v of obj) {
  result.push(v);
}
console.log(result.join(","));
