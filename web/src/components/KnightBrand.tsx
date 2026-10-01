import pearl from "../assets/knight-pearl.svg";
import trio from "../assets/knight-trio.svg";

export function KnightBrand({ wordmark = false }: { wordmark?: boolean }) {
  return wordmark ? (
    <span className="themis-wordmark" aria-label="ThemisCode">
      ThemisC<img src={pearl} alt="" />de
    </span>
  ) : <img className="themis-app-mark" src={trio} alt="ThemisCode" />;
}
