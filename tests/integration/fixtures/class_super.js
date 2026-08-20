// @expected-stdout: Rex barks loudly
// super() call in derived constructor + super.method() property access
class Animal {
  constructor(name) {
    this.name = name;
  }
  speak() {
    return this.name + " speaks";
  }
}
class Dog extends Animal {
  constructor(name) {
    super(name);
    this.sound = "barks";
  }
  speak() {
    return this.name + " " + this.sound + " loudly";
  }
}
let d = new Dog("Rex");
console.log(d.speak());
