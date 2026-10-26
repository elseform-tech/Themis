const items = [];
const list = document.getElementById("list");
const input = document.getElementById("new-item");

function render() {
  list.innerHTML = "";
  items.forEach((item) => {
    const li = document.createElement("li");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = item.done;
    box.addEventListener("change", () => {
      item.done = box.checked;
      render();
    });
    const label = document.createElement("span");
    label.textContent = item.text;
    li.appendChild(box);
    li.appendChild(label);
    list.appendChild(li);
  });
  document.getElementById("count").textContent = `${items.filter((item) => !item.done).length} left`;
}

document.getElementById("add").addEventListener("click", () => {
  const text = input.value.trim();
  if (!text) return;
  items.push({ text, done: false });
  input.value = "";
  render();
});

document.getElementById("clear-done").addEventListener("click", () => {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (items[index].done) items.splice(index, 1);
  }
  render();
});

render();
